// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

use std::{collections::BTreeMap, time::Duration};

use sc_network::PeerId;
use sc_network_gossip::{ValidationResult, Validator, ValidatorContext};
use sp_runtime::traits::{Block, NumberFor};

use codec::Decode;
use log::{debug, error, trace};
use parking_lot::{Mutex, RwLock};
use wasm_timer::Instant;

use crate::types::dkg_topic;
use dkg_primitives::types::{DKGMessage, DKGPayloadKey};
use dkg_runtime_primitives::{crypto::Public, ChainId, MmrRootHash};

// Limit DKG gossip by keeping only a bound number of voting rounds alive.
const MAX_LIVE_GOSSIP_ROUNDS: usize = 3;

// Timeout for rebroadcasting messages.
const REBROADCAST_AFTER: Duration = Duration::from_secs(60 * 5);

/// A type that represents hash of the message.
pub type MessageHash = [u8; 8];

type KnownVotes<B> = BTreeMap<NumberFor<B>, fnv::FnvHashSet<MessageHash>>;

/// DKG gossip validator
///
/// Validate DKG gossip messages and limit the number of live DKG voting rounds.
///
/// Allows messages from last [`MAX_LIVE_GOSSIP_ROUNDS`] to flow, everything else gets
/// rejected/expired.
///
/// All messaging is handled in a single DKG global topic.
pub(crate) struct GossipValidator<B>
where
	B: Block,
{
	topic: B::Hash,
	known_votes: RwLock<KnownVotes<B>>,
	next_rebroadcast: Mutex<Instant>,
}

impl<B> GossipValidator<B>
where
	B: Block,
{
	pub fn new() -> GossipValidator<B> {
		GossipValidator {
			topic: dkg_topic::<B>(),
			known_votes: RwLock::new(BTreeMap::new()),
			next_rebroadcast: Mutex::new(Instant::now() + REBROADCAST_AFTER),
		}
	}

	/// Note a voting round.
	///
	/// Noting `round` will keep `round` live.
	///
	/// We retain the [`MAX_LIVE_GOSSIP_ROUNDS`] most **recent** voting rounds as live.
	/// As long as a voting round is live, it will be gossiped to peer nodes.
	pub(crate) fn note_round(&self, round: NumberFor<B>) {
		debug!(target: "dkg", "🕸️  About to note round #{}", round);

		let mut live = self.known_votes.write();

		#[allow(clippy::map_entry)]
		if !live.contains_key(&round) {
			live.insert(round, Default::default());
		}

		if live.len() > MAX_LIVE_GOSSIP_ROUNDS {
			let to_remove = live.iter().next().map(|x| x.0).copied();
			if let Some(first) = to_remove {
				live.remove(&first);
			}
		}
	}

	fn add_known(known_votes: &mut KnownVotes<B>, round: &NumberFor<B>, hash: MessageHash) {
		known_votes.get_mut(round).map(|known| known.insert(hash));
	}

	// Note that we will always keep the most recent unseen round alive.
	//
	// This is a preliminary fix and the detailed description why we are
	// doing this can be found as part of the issue below
	//
	// https://github.com/paritytech/grandpa-bridge-gadget/issues/237
	//
	fn is_live(known_votes: &KnownVotes<B>, round: &NumberFor<B>) -> bool {
		let unseen_round = if let Some(max_known_round) = known_votes.keys().last() {
			round > max_known_round
		} else {
			known_votes.is_empty()
		};

		known_votes.contains_key(round) || unseen_round
	}

	fn is_known(known_votes: &KnownVotes<B>, round: &NumberFor<B>, hash: &MessageHash) -> bool {
		known_votes.get(round).map(|known| known.contains(hash)).unwrap_or(false)
	}
}

impl<B> Validator<B> for GossipValidator<B>
where
	B: Block,
{
	fn validate(
		&self,
		_context: &mut dyn ValidatorContext<B>,
		sender: &PeerId,
		data: &[u8],
	) -> ValidationResult<B::Hash> {
		let mut data_copy = data;
		trace!(target: "dkg", "🕸️  Got a message: {:?}, from: {:?}", data_copy, sender);
		match DKGMessage::<Public>::decode(&mut data_copy) {
			Ok(msg) => {
				trace!(target: "dkg", "🕸️  Got dkg message: {:?}, from: {:?}", msg, sender);
				return ValidationResult::ProcessAndKeep(dkg_topic::<B>())
			},
			Err(e) => {
				error!(target: "dkg", "🕸️  Got invalid dkg message: {:?}, from: {:?}", e, sender);
			},
		}

		ValidationResult::Discard
	}
}
