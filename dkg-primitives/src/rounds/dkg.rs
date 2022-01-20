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

use super::{keygen::*, offline::*, sign::*};


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

/// DKG State tracker
pub struct DKGState<C> where C: AtLeast32BitUnsigned + Copy  {
	pub accepted: bool,
	pub is_epoch_over: bool,
	pub listening_for_pub_key: bool,
	pub listening_for_active_pub_key: bool,
	pub curr_dkg: Option<MultiPartyECDSARounds<C>>,
	pub past_dkg: Option<MultiPartyECDSARounds<C>>,
	pub created_offlinestage_at: HashMap<Vec<u8>, C>,
}

/// State machine structure for performing Keygen, Offline stage and Sign rounds
/// HashMap and BtreeMap keys are encoded formats of (ChainId, DKGPayloadKey)
/// Using DKGPayloadKey only will cause collisions when proposals with the same nonce but from
/// different chains are submitted
pub struct MultiPartyECDSARounds<Clock> where Clock: AtLeast32BitUnsigned + Copy {
	round_id: RoundId,
	party_index: u16,
	threshold: u16,
	parties: u16,

	created_at: Clock,

	// Key generation
	keygen: KeygenState<Clock>,
	// Offline stage
	offlines: HashMap<Vec<u8>, OfflineState<Clock>>,
	// Signing rounds
	votes: HashMap<Vec<u8>, SignState<Clock>>,

	keygen_set_id: KeygenSetId,
	signers: Vec<u16>,
	signer_set_id: SignerSetId,

	// File system storage and encryption
	local_key_path: Option<PathBuf>,
	public_key: Option<sr25519::Public>,
	local_keystore: Option<Arc<LocalKeystore>>,
}

impl<C> MultiPartyECDSARounds<C>
where
	C: AtLeast32BitUnsigned + Copy,
{
	/// Public ///

	pub fn new(
		round_id: RoundId,
		party_index: u16,
		threshold: u16,
		parties: u16,
		local_key_path: Option<PathBuf>,
		created_at: C,
		public_key: Option<sr25519::Public>,
		local_keystore: Option<Arc<LocalKeystore>>,
	) -> Self {
		trace!(target: "dkg", "🕸️  Creating new MultiPartyECDSARounds, party_index: {}, threshold: {}, parties: {}", party_index, threshold, parties);

		Self {
			round_id,
			party_index,
			threshold,
			parties,
			created_at,
			keygen: KeygenState::NotStarted(PreKeygenRounds::new(round_id)),
			offlines: HashMap::new(),
			votes: HashMap::new(),
			keygen_set_id: 0,
			signers: Vec::new(),
			signer_set_id: 0,
			local_key_path,
			public_key,
			local_keystore,
		}
	}

	pub fn set_local_key(&mut self, local_key: LocalKey<Secp256k1>) {
		self.keygen = KeygenState::Finished(Ok(local_key));
	}

	pub fn set_signers(&mut self, signers: Vec<u16>) {
		self.signers = signers;
	}

	pub fn set_signer_set_id(&mut self, set_id: SignerSetId) {
		self.signer_set_id = set_id;
	}

	/// A check to know if the protocol has stalled at the keygen stage,
	/// We take it that the protocol has stalled if keygen messages are not received from other peers after a certain interval
	/// And the keygen stage has not completed
	pub fn has_stalled(&self, time_to_restart: Option<C>, current_block_number: C) -> bool {

		false
	}

	pub fn proceed(&mut self, at: C) -> Vec<Result<(), DKGError>> {
		// let proceed_res = match self.stage {
		// 	Stage::Keygen => self.proceed_keygen(at),
		// 	Stage::OfflineReady => Ok(false),
		// 	_ => Ok(false),
		// };

		// let mut results = vec![];

		// match proceed_res {
		// 	Ok(finished) =>
		// 		if finished {
		// 			self.advance_stage();
		// 		},
		// 	Err(err) => results.push(Err(err)),
		// }

		// let keys = self.offline_stage.keys().cloned().collect::<Vec<_>>();
		// for key in &keys {
		// 	let res = self.proceed_offline_stage(key.clone(), at).map(|_| ());
		// 	if res.is_err() {
		// 		results.push(res);
		// 	}
		// }

		// let res = self.proceed_vote(at).map(|_| ());

		// if res.is_err() {
		// 	results.push(res);
		// }

		// results
		vec![]
	}

	pub fn get_outgoing_messages(&mut self) -> Vec<DKGMsgPayload> {
		trace!(target: "dkg", "🕸️  Get outgoing messages");

		let mut all_messages: Vec<DKGMsgPayload> = self.keygen
			.get_outgoing()
			.into_iter()
			.map(|msg| DKGMsgPayload::Keygen(msg))
			.collect();

		let offline_messages = self.offlines
			.values()
			.map(|s| s.get_outgoing())
			.fold(Vec::new(), |acc, x| { acc.extend(x); acc })
			.into_iter()
			.map(|msg| DKGMsgPayload::Offline(msg))
			.collect::<Vec<_>>();

		let vote_messages = self.votes
			.values()
			.map(|s| s.get_outgoing())
			.fold(Vec::new(), |acc, x| { acc.extend(x); acc })
			.into_iter()
			.map(|msg| DKGMsgPayload::Vote(msg))
			.collect::<Vec<_>>();

		all_messages.extend_from_slice(&offline_messages[..]);
		all_messages.extend_from_slice(&vote_messages[..]);

		all_messages
	}

	pub fn handle_incoming(
		&mut self,
		data: DKGMsgPayload,
		at: Option<C>,
	) -> Result<(), DKGError> {
		trace!(target: "dkg", "🕸️  Handle incoming");

		return match data {
			DKGMsgPayload::Keygen(msg) => {
				self.keygen.handle_incoming(msg, at.unwrap())

			},
			DKGMsgPayload::Offline(msg) => {
				let key = msg.key.clone();

				let offline = self.offlines.entry(key.clone()).or_insert_with(|| {
					OfflineState::NotStarted(PreOfflineRounds::new(self.signer_set_id))
				});

				let res = offline.handle_incoming(msg, at.unwrap());
				if let Err(DKGError::CriticalError { reason: _ }) = res.clone() {
					self.offlines.remove(&key);
				}
				res
			},
			DKGMsgPayload::Vote(msg) => {
				let key = msg.round_key.clone();

				let vote = self.votes.entry(key.clone()).or_insert_with(|| {
					SignState::NotStarted(PreSignRounds::new(self.signer_set_id))
				});

				let res = vote.handle_incoming(msg, at.unwrap());
				if let Err(DKGError::CriticalError { reason: _ }) = res.clone() {
					self.votes.remove(&key);
				}
				res
			},
			_ => Ok(()),
		}
	}

	pub fn start_keygen(
		&mut self,
		keygen_set_id: KeygenSetId,
		started_at: C,
	) -> Result<(), DKGError> {
		info!(
			target: "dkg",
			"🕸️  Starting new DKG w/ party_index {:?}, threshold {:?}, size {:?}",
			self.party_index,
			self.threshold,
			self.parties,
		);
		trace!(target: "dkg", "🕸️  Keygen set id: {}", keygen_set_id);

		match self.keygen {
			KeygenState::NotStarted(pre_keygen) => {
				match Keygen::new(self.party_index, self.threshold, self.parties) {
					Ok(new_keygen) => {
						self.keygen = KeygenState::Started(
							KeygenRounds::new(
								self.keygen_params(),
								started_at,
								new_keygen
							));
		
						// Processing pending messages
						for msg in std::mem::take(&mut pre_keygen.pending_keygen_msgs) {
							if let Err(err) = self.keygen.handle_incoming(msg, started_at) {
								warn!(target: "dkg", "🕸️  Error handling pending keygen msg {:?}", err);
							}
							self.keygen.proceed(started_at)?;
						}
						trace!(target: "dkg", "🕸️  Handled {} pending keygen messages", &pre_keygen.pending_keygen_msgs.len());
		
						Ok(())
					},
					Err(err) => Err(DKGError::StartKeygen { reason: err.to_string() }),
				}
			},
			_ => Err(DKGError::StartKeygen { reason: "Already started".to_string() }),
		}
	}

	pub fn create_offline_stage(&mut self, key: Vec<u8>, started_at: C) -> Result<(), DKGError> {
		info!(target: "dkg", "🕸️  Creating offline stage for {:?}", &key);
		match self.keygen {
			KeygenState::Finished(Ok(local_key)) => {

				let s_l = (1..=self.dkg_params().2).collect::<Vec<_>>();
				return match OfflineStage::new(self.party_index, s_l.clone(), local_key.clone()) {
					Ok(new_offline_stage) => {
						let offline = self.offlines.entry(key.clone()).or_insert_with(|| {
							OfflineState::NotStarted(PreOfflineRounds::new(self.signer_set_id))
						});

						match offline {
							OfflineState::NotStarted(pre_offline) => {
								let new_offline = OfflineState::Started(
									OfflineRounds::new(
										self.sign_params(),
										started_at,
										key.clone(),
										new_offline_stage
									));

								for msg in pre_offline.pending_offline_msgs {
									if let Err(err) = new_offline.handle_incoming(msg, started_at) {
										warn!(target: "dkg", "🕸️  Error handling pending offline msg {:?}", err);
									}
									new_offline.proceed(started_at)?;
								}
								trace!(target: "dkg", "🕸️  Handled pending offline messages for {:?}", key);
								
								self.offlines.insert(key.clone(), new_offline);

								Ok(())
							},
							_ => Err(DKGError::CreateOfflineStage { reason: "Already started".to_string() })
						}
					},
					Err(err) => {
						error!("Error creating new offline stage {}", err);
						Err(DKGError::CreateOfflineStage { reason: err.to_string() })
					},
				}
			},
			_ => Err(DKGError::CreateOfflineStage {
				reason: "Cannot start offline stage, Keygen is not complete".to_string(),
			}),
		}
	}

	pub fn vote(&mut self, round_key: Vec<u8>, data: Vec<u8>, started_at: C) -> Result<(), DKGError> {
		if let Some(OfflineState::Finished(Ok(completed_offline))) = self.offlines.remove(&round_key) {
			let hash = BigInt::from_bytes(&keccak_256(&data));

			match SignManual::new(hash, completed_offline.clone()) {
				Ok((sign_manual, sig)) => {
					trace!(target: "dkg", "🕸️  Creating vote /w key {:?}", &round_key);

					let vote = self.votes.entry(round_key.clone()).or_insert_with(|| {
						SignState::NotStarted(PreSignRounds::new(self.signer_set_id))
					});

					match vote {
						SignState::NotStarted(pre_sign) => {
							let new_sign = SignState::Started(
								SignRounds::new(
									self.sign_params(),
									started_at,
									data,
									round_key.clone(),
									sig,
									sign_manual
								));
							
							for msg in pre_sign.pending_sign_msgs {
								if let Err(err) = new_sign.handle_incoming(msg, started_at) {
									warn!(target: "dkg", "🕸️  Error handling pending vote msg {:?}", err);
								}
								new_sign.proceed(started_at)?;
							}
							trace!(target: "dkg", "🕸️  Handled pending vote messages for {:?}", key);
							
							self.votes.insert(key.clone(), new_sign);

							Ok(())
						},
						_ => Err(DKGError::Vote { reason: "Already started".to_string() })
					};
					
					Ok(())
				},
				Err(err) => Err(DKGError::Vote { reason: err.to_string() }),
			}
		} else {
			Err(DKGError::Vote { reason: "Not ready to vote".to_string() })
		}
	}

	pub fn is_key_gen_stage(&self) -> bool {
		match self.keygen {
			KeygenState::Finished(_) => false,
			_ => true,
		}
	}

	pub fn is_offline_ready(&self) -> bool {
		match self.keygen {
			KeygenState::Finished(_) => true,
			_ => false,
		}
	}

	pub fn is_ready_to_vote(&self, key: Vec<u8>) -> bool {
		if let Some(offline) = self.offlines.get(&key) {
			match offline {
				OfflineState::Finished(_) => true,
				_ => false,
			}
		} else {
			false
		}
	}

	pub fn has_finished_rounds(&self) -> bool {
		let finished = self.votes.values().filter(|v| match {
			SignState::Finished(_) => true,
			_ => false,
		})
		.count();

		finished > 0
	}

	pub fn get_finished_rounds(&mut self) -> Vec<DKGSignedPayload> {
		// std::mem::take(&mut self.finished_rounds)
		vec![]
	}

	pub fn dkg_params(&self) -> (u16, u16, u16) {
		(self.party_index, self.threshold, self.parties)
	}

	pub fn is_signer(&self) -> bool {
		self.signers.contains(&self.party_index)
	}

	pub fn get_public_key(&self) -> Option<GE> {
		match self.keygen {
			KeygenState::Finished(Ok(local_key)) => {
				Some(local_key.public_key().clone())
			},
			_ => None,
		}
	}

	pub fn get_id(&self) -> RoundId {
		self.round_id
	}

	pub fn has_vote_in_process(&self, round_key: Vec<u8>) -> bool {
		return self.votes.contains_key(&round_key)
	}

	/// Utils

	fn keygen_params(&self) -> KeygenParams {
		KeygenParams {
			round_id: self.round_id,
			party_index: self.party_index,
			threshold: self.threshold,
			parties: self.parties,
			keygen_set_id: self.keygen_set_id,
		}
	}

	fn sign_params(&self) -> SignParams {
		SignParams {
			round_id: self.round_id,
			party_index: self.party_index,
			threshold: self.threshold,
			parties: self.parties,
			signer_set_id: self.signer_set_id,
			signers: self.signers,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::{MultiPartyECDSARounds, Stage};
	use codec::Encode;

	fn check_all_reached_stage(
		parties: &Vec<MultiPartyECDSARounds<u32>>,
		target_stage: Stage,
	) -> bool {
		for party in parties.iter() {
			if party.stage != target_stage {
				return false
			}
		}
		true
	}

	fn check_all_parties_have_public_key(parties: &Vec<MultiPartyECDSARounds<u32>>) {
		for party in parties.iter() {
			if party.get_public_key().is_none() {
				panic!("No public key for party {}", party.party_index)
			}
		}
	}

	fn check_all_reached_offline_ready(parties: &Vec<MultiPartyECDSARounds<u32>>) -> bool {
		check_all_reached_stage(parties, Stage::OfflineReady)
	}

	fn check_all_reached_manual_ready(parties: &Vec<MultiPartyECDSARounds<u32>>) -> bool {
		let round_key = 1u32.encode();
		for party in parties.iter() {
			if !party.is_ready_to_vote(round_key.clone()) {
				return false
			}
		}
		true
	}

	fn check_all_signatures_ready(parties: &Vec<MultiPartyECDSARounds<u32>>) -> bool {
		for party in parties.iter() {
			if !party.is_signer() {
				continue
			}
			if !party.has_finished_rounds() {
				return false
			}
		}
		true
	}

	fn check_all_signatures_correct(parties: &mut Vec<MultiPartyECDSARounds<u32>>) {
		for party in &mut parties.into_iter() {
			let mut finished_rounds = party.get_finished_rounds();

			if finished_rounds.len() == 1 {
				let finished_round = finished_rounds.remove(0);

				let message = b"Webb".encode();

				assert!(
					dkg_runtime_primitives::utils::validate_ecdsa_signature(
						&message,
						&finished_round.signature
					),
					"Invalid signature for party {}",
					party.party_index
				);

				println!("Party {}; sig: {:?}", party.party_index, &finished_round.signature);
			} else {
				panic!("No signature extracted")
			}
		}

		println!("All signatures are correct");
	}

	fn run_simulation<C>(parties: &mut Vec<MultiPartyECDSARounds<u32>>, stop_condition: C)
	where
		C: Fn(&Vec<MultiPartyECDSARounds<u32>>) -> bool,
	{
		println!("Simulation starts");

		let mut msgs_pull = vec![];

		for party in &mut parties.into_iter() {
			party.proceed(0);

			msgs_pull.append(&mut party.get_outgoing_messages());
		}

		for _i in 1..100 {
			let msgs_pull_frozen = msgs_pull.split_off(0);

			for party in &mut parties.into_iter() {
				for msg_frozen in msgs_pull_frozen.iter() {
					match party.handle_incoming(msg_frozen.clone(), None) {
						Ok(()) => (),
						Err(err) => panic!("{:?}", err),
					}
				}
				msgs_pull.append(&mut party.get_outgoing_messages());
			}

			for party in &mut parties.into_iter() {
				party.proceed(0);

				msgs_pull.append(&mut party.get_outgoing_messages());
			}

			if stop_condition(parties) {
				println!("All parties finished");
				return
			}
		}
	}

	fn simulate_multi_party(t: u16, n: u16) {
		let mut parties: Vec<MultiPartyECDSARounds<u32>> = vec![];
		let round_key = 1u32.encode();
		for i in 1..=n {
			let mut party = MultiPartyECDSARounds::new(i, t, n, 0, None, 0, None, None);
			println!("Starting keygen for party {}, Stage: {:?}", party.party_index, party.stage);
			party.start_keygen(0, 0).unwrap();
			parties.push(party);
		}

		// Running Keygen stage
		println!("Running Keygen");
		run_simulation(&mut parties, check_all_reached_offline_ready);
		check_all_parties_have_public_key(&mut &parties);

		// Running Offline stage
		println!("Running Offline");
		let parties_refs = &mut parties;
		for party in parties_refs.into_iter() {
			println!("Creating offline stage");
			match party.create_offline_stage(round_key.clone(), 0) {
				Ok(()) => (),
				Err(_err) => (),
			}
		}
		run_simulation(&mut parties, check_all_reached_manual_ready);

		// Running Sign stage
		println!("Running Sign");
		let parties_refs = &mut parties;
		for party in &mut parties_refs.into_iter() {
			println!("Vote for party {}", party.party_index);
			party.vote(round_key.clone(), "Webb".encode(), 0).unwrap();
		}
		run_simulation(&mut parties, check_all_signatures_ready);

		// Extract all signatures and check for correctness
		check_all_signatures_correct(&mut parties);
	}

	#[test]
	fn simulate_multi_party_t2_n3() {
		simulate_multi_party(2, 3);
	}

	#[test]
	fn simulate_multi_party_t3_n5() {
		simulate_multi_party(3, 5);
	}
}
