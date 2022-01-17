use codec::Encode;
use curv::{arithmetic::Converter, elliptic::curves::Secp256k1, BigInt};
use log::{debug, error, info, trace, warn};
use round_based::{IsCritical, Msg, StateMachine};
use sc_keystore::LocalKeystore;
use sp_core::{ecdsa::Signature, sr25519, Pair as TraitPair};
use sp_runtime::traits::AtLeast32BitUnsigned;
use std::{
	collections::{BTreeMap, HashMap},
	path::PathBuf,
	sync::Arc,
};

use crate::{
	types::*,
	utils::{select_random_set, store_localkey, vec_usize_to_u16},
};
use dkg_runtime_primitives::{
	keccak_256,
	offchain_crypto::{Pair as AppPair, Public},
};

pub use gg_2020::{
	party_i::*,
	state_machine::{keygen::*, sign::*},
};
pub use multi_party_ecdsa::protocols::multi_party_ecdsa::{
	gg_2020,
	gg_2020::state_machine::{keygen as gg20_keygen, sign as gg20_sign, traits::RoundBlame},
};

pub enum KeygenState<C> {
	NotStarted(PreKeygenRounds<C>),
	Started(KeygenRounds<C>),
	Finished(Result<LocalKey<Secp256k1>, DKGError>),
}

/// Pre-keygen rounds
pub struct PreKeygenRounds<Clock> {
	round_id: RoundId,
	pending_keygen_msgs: Vec<DKGKeygenMessage>,
}

impl<C> PreKeygenRounds<C>
where
	C: AtLeast32BitUnsigned + Copy,
{
	pub fn new(round_id: RoundId) -> Self {
		Self{
			round_id,
			pending_keygen_msgs: Vec::default(),
		}
	}
}

impl<C> DKGRoundsSM<DKGKeygenMessage, Vec<DKGKeygenMessage>, C> for PreKeygenRounds<C>
where
	C: AtLeast32BitUnsigned + Copy,
{
	pub fn handle_incoming(&mut self, data: DKGKeygenMessage) -> Result<(), DKGError> {
		self.pending_keygen_msgs.push(data);
		Ok(())
	}

	pub fn is_finished(&self) {
		true
	}
	
	pub fn try_finish(self) -> Result<Vec<DKGKeygenMessage>, DKGError> {
		Ok(self.pending_keygen_msgs.take())
	}
}

/// Keygen rounds

pub struct KeygenRounds<Clock> {
	round_id: RoundId,

	party_index: u16,
	threshold: u16,
	parties: u16,

	keygen_set_id: KeygenSetId,
	// DKG clock
	keygen_started_at: Clock,
	// Key generation
	keygen: Keygen,
	output: Option<Result<LocalKey<Secp256k1>, DKGError>>,
}

impl<C> KeygenRounds<C>
where
	C: AtLeast32BitUnsigned + Copy,
{
	/// Proceed to next step for current Stage

	pub fn proceed(&mut self, at: C) -> Result<bool, DKGError> {
		trace!(target: "dkg", "🕸️  Keygen party {} enter proceed", self.party_index);

		let keygen = &mut self.keygen;

		if keygen.wants_to_proceed() {
			info!(target: "dkg", "🕸️  Keygen party {} wants to proceed", keygen.party_ind());
			trace!(target: "dkg", "🕸️  before: {:?}", keygen);

			match keygen.proceed() {
				Ok(_) => {
					trace!(target: "dkg", "🕸️  after: {:?}", keygen);
				},
				Err(err) => {
					match err {
						gg20_keygen::Error::ProceedRound(proceed_err) => match proceed_err {
							gg20_keygen::ProceedError::Round2VerifyCommitments(err_type) =>
								return Err(DKGError::KeygenMisbehaviour {
									bad_actors: vec_usize_to_u16(err_type.bad_actors),
								}),
							gg20_keygen::ProceedError::Round3VerifyVssConstruct(err_type) =>
								return Err(DKGError::KeygenMisbehaviour {
									bad_actors: vec_usize_to_u16(err_type.bad_actors),
								}),
							gg20_keygen::ProceedError::Round4VerifyDLogProof(err_type) =>
								return Err(DKGError::KeygenMisbehaviour {
									bad_actors: vec_usize_to_u16(err_type.bad_actors),
								}),
						},
						_ => return Err(DKGError::GenericError { reason: err.to_string() }),
					};
				},
			}
		}

		let (_, blame_vec) = keygen.round_blame();

		if self.try_finish_keygen() {
			Ok(true)
		} else {
			if at - self.keygen_started_at > KEYGEN_TIMEOUT.into() {
				if !blame_vec.is_empty() {
					return Err(DKGError::KeygenTimeout { bad_actors: blame_vec })
				} else {
					// Should never happen
					warn!(target: "dkg", "🕸️  Keygen timeout reached, but no missing parties found", );
				}
			}
			Ok(false)
		}
	}

	/// Try finish current Stage

	fn try_finish(&mut self) -> bool {
		let keygen = self.keygen.as_mut().unwrap();

		if keygen.is_finished() {
			info!(target: "dkg", "🕸️  Keygen is finished, extracting output, round_id: {:?}", self.round_id);
			match keygen.pick_output() {
				Some(Ok(k)) => {
					self.local_key = Some(k.clone());
					info!(target: "dkg", "🕸️  local share key is extracted");
					return true
				},
				Some(Err(e)) => panic!("Keygen finished with error result {}", e),
				None => panic!("Keygen finished with no result"),
			}
		}
		return false
	}

	/// Get outgoing messages for current Stage

	pub fn get_outgoing_messages(&mut self) -> Vec<DKGKeygenMessage> {
		if let Some(keygen) = self.keygen.as_mut() {
			trace!(target: "dkg", "🕸️  Getting outgoing keygen messages");

			if !keygen.message_queue().is_empty() {
				trace!(target: "dkg", "🕸️  Outgoing messages, queue len: {}", keygen.message_queue().len());

				let keygen_set_id = self.keygen_set_id;

				let enc_messages = keygen
					.message_queue()
					.into_iter()
					.map(|m| {
						trace!(target: "dkg", "🕸️  MPC protocol message {:?}", m);
						let serialized = serde_json::to_string(&m).unwrap();
						return DKGKeygenMessage {
							keygen_set_id,
							keygen_msg: serialized.into_bytes(),
						}
					})
					.collect::<Vec<DKGKeygenMessage>>();

				keygen.message_queue().clear();
				return enc_messages
			}
		}
		vec![]
	}

	/// Handle incoming messages for current Stage

	pub fn handle_incoming(&mut self, data: DKGKeygenMessage) -> Result<(), DKGError> {
		if data.keygen_set_id != self.keygen_set_id {
			return Err(DKGError::GenericError { reason: "Keygen set ids do not match".to_string() })
		}

		if let Some(keygen) = self.keygen.as_mut() {
			trace!(target: "dkg", "🕸️  Handle incoming keygen message");
			println!("🕸️  Handle incoming keygen message: {:?}", data);
			if data.keygen_msg.is_empty() {
				warn!(
					target: "dkg", "🕸️  Got empty message");
				return Ok(())
			}

			let msg: Msg<ProtocolMessage> = match serde_json::from_slice(&data.keygen_msg) {
				Ok(msg) => msg,
				Err(err) => {
					error!(target: "dkg", "🕸️  Error deserializing msg: {:?}", err);
					println!("🕸️  Error deserializing msg: {:?}", err);
					return Err(DKGError::GenericError {
						reason: "Error deserializing keygen msg".to_string(),
					})
				},
			};

			if Some(keygen.party_ind()) != msg.receiver &&
				(msg.receiver.is_some() || msg.sender == keygen.party_ind())
			{
				warn!(target: "dkg", "🕸️  Ignore messages sent by self");
				return Ok(())
			}
			trace!(
				target: "dkg", "🕸️  Party {} got message from={}, broadcast={}: {:?}",
				keygen.party_ind(),
				msg.sender,
				msg.receiver.is_none(),
				msg.body,
			);
			debug!(target: "dkg", "🕸️  State before incoming message processing: {:?}", keygen);
			match keygen.handle_incoming(msg.clone()) {
				Ok(()) => (),
				Err(err) if err.is_critical() => {
					error!(target: "dkg", "🕸️  Critical error encountered: {:?}", err);
					return Err(DKGError::GenericError {
						reason: "Keygen critical error encountered".to_string(),
					})
				},
				Err(err) => {
					error!(target: "dkg", "🕸️  Non-critical error encountered: {:?}", err);
				},
			}
			debug!(target: "dkg", "🕸️  State after incoming message processing: {:?}", keygen);
		}
		Ok(())
	}
}
