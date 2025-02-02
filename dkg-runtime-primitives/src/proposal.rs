use sp_std::hash::{Hash, Hasher};

use codec::{Decode, Encode};
use sp_std::vec::Vec;

pub const PROPOSAL_SIGNATURE_LENGTH: usize = 65;

pub type ResourceId = [u8; 32];
// Proposal Nonces (4 bytes)
pub type ProposalNonce = u32;

#[derive(Clone, Encode, Decode, scale_info::TypeInfo)]
pub struct RefreshProposal {
	pub nonce: ProposalNonce,
	pub pub_key: Vec<u8>,
}

#[derive(
	Eq, PartialEq, Clone, Encode, Decode, scale_info::TypeInfo, frame_support::RuntimeDebug,
)]
pub struct RefreshProposalSigned {
	pub nonce: ProposalNonce,
	pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, scale_info::TypeInfo)]
pub struct ProposalHeader {
	pub resource_id: ResourceId,
	pub chain_id: u32,
	pub function_sig: [u8; 4],
	pub nonce: ProposalNonce,
}

impl Encode for ProposalHeader {
	fn encode(&self) -> Vec<u8> {
		let mut buf = Vec::new();
		// resource_id contains the chain id already.
		buf.extend_from_slice(&self.resource_id); // 32 bytes
		buf.extend_from_slice(&self.function_sig); // 4 bytes
		// we encode it in big endian that is what EVM expect things to be.
		buf.extend_from_slice(&self.nonce.to_be_bytes()); // 4 bytes
		buf
	}

	fn encoded_size(&self) -> usize {
		40 // Bytes
	}
}

impl Decode for ProposalHeader {
	fn decode<I: codec::Input>(input: &mut I) -> Result<Self, codec::Error> {
		let mut data = [0u8; 40];
		input.read(&mut data).map_err(|_| {
			codec::Error::from("input bytes are less than the header size (40 bytes)")
		})?;
		// _NOTE_: rustc won't generate bounds check for the following slice
		// since we know the length of the slice is at least 40 bytes already.

		// decode the resourceId is the first 32 bytes
		let mut resource_id = [0u8; 32];
		resource_id.copy_from_slice(&data[0..32]);
		// the chain id is the last 4 bytes of the **resourceId**
		let mut chain_id_bytes = [0u8; 4];
		chain_id_bytes.copy_from_slice(&resource_id[28..32]);
		let chain_id = u32::from_be_bytes(chain_id_bytes);
		// the function signature is the next first 4 bytes after the resourceId.
		let mut function_sig = [0u8; 4];
		function_sig.copy_from_slice(&data[32..36]);
		// the nonce is the last 4 bytes of the header (also considered as the first arg).
		let mut nonce_bytes = [0u8; 4];
		nonce_bytes.copy_from_slice(&data[36..40]);
		let nonce = u32::from_be_bytes(nonce_bytes);
		let header = ProposalHeader {
			resource_id,
			chain_id,
			function_sig,
			nonce: ProposalNonce::from(nonce),
		};
		Ok(header)
	}
}

impl From<ProposalHeader> for (u32, ProposalNonce) {
	fn from(header: ProposalHeader) -> Self {
		(header.chain_id, header.nonce)
	}
}

impl From<ProposalHeader> for (ResourceId, u32, ProposalNonce) {
	fn from(header: ProposalHeader) -> Self {
		(header.resource_id, header.chain_id, header.nonce)
	}
}

#[derive(Debug, Clone, Decode, Encode, Copy, Eq, PartialOrd, Ord, scale_info::TypeInfo)]
pub enum DKGPayloadKey {
	EVMProposal(ProposalNonce),
	RefreshVote(ProposalNonce),
	AnchorUpdateProposal(ProposalNonce),
	TokenAddProposal(ProposalNonce),
	TokenRemoveProposal(ProposalNonce),
	WrappingFeeUpdateProposal(ProposalNonce),
	ResourceIdUpdateProposal(ProposalNonce),
}

impl PartialEq for DKGPayloadKey {
	fn eq(&self, other: &Self) -> bool {
		match (self, other) {
			(Self::EVMProposal(l0), Self::EVMProposal(r0)) => l0 == r0,
			(Self::RefreshVote(l0), Self::RefreshVote(r0)) => l0 == r0,
			(Self::AnchorUpdateProposal(l0), Self::AnchorUpdateProposal(r0)) => l0 == r0,
			(Self::TokenAddProposal(l0), Self::TokenAddProposal(r0)) => l0 == r0,
			(Self::TokenRemoveProposal(l0), Self::TokenRemoveProposal(r0)) => l0 == r0,
			(Self::WrappingFeeUpdateProposal(l0), Self::WrappingFeeUpdateProposal(r0)) => l0 == r0,
			(Self::ResourceIdUpdateProposal(l0), Self::ResourceIdUpdateProposal(r0)) => l0 == r0,
			_ => false,
		}
	}
}

impl Hash for DKGPayloadKey {
	fn hash<H: Hasher>(&self, state: &mut H) {
		self.encode().hash(state)
	}
}
pub enum ProposalAction {
	// sign the proposal with some priority
	Sign(u8),
}

#[derive(Debug, Encode, Decode, Clone, Eq, PartialEq, scale_info::TypeInfo)]
pub enum ProposalType {
	RefreshProposal { data: Vec<u8> },
	EVMUnsigned { data: Vec<u8> },
	EVMSigned { data: Vec<u8>, signature: Vec<u8> },
	AnchorUpdate { data: Vec<u8> },
	AnchorUpdateSigned { data: Vec<u8>, signature: Vec<u8> },
	TokenAdd { data: Vec<u8> },
	TokenAddSigned { data: Vec<u8>, signature: Vec<u8> },
	TokenRemove { data: Vec<u8> },
	TokenRemoveSigned { data: Vec<u8>, signature: Vec<u8> },
	WrappingFeeUpdate { data: Vec<u8> },
	WrappingFeeUpdateSigned { data: Vec<u8>, signature: Vec<u8> },
	ResourceIdUpdate { data: Vec<u8> },
	ResourceIdUpdateSigned { data: Vec<u8>, signature: Vec<u8> },
}

impl ProposalType {
	pub fn data(&self) -> Vec<u8> {
		match self {
			ProposalType::RefreshProposal { data } => data.clone(),
			ProposalType::EVMUnsigned { data } => data.clone(),
			ProposalType::EVMSigned { data, .. } => data.clone(),
			ProposalType::AnchorUpdate { data } => data.clone(),
			ProposalType::AnchorUpdateSigned { data, .. } => data.clone(),
			ProposalType::TokenAdd { data } => data.clone(),
			ProposalType::TokenAddSigned { data, .. } => data.clone(),
			ProposalType::TokenRemove { data } => data.clone(),
			ProposalType::TokenRemoveSigned { data, .. } => data.clone(),
			ProposalType::WrappingFeeUpdate { data } => data.clone(),
			ProposalType::WrappingFeeUpdateSigned { data, .. } => data.clone(),
			ProposalType::ResourceIdUpdate { data } => data.clone(),
			ProposalType::ResourceIdUpdateSigned { data, .. } => data.clone(),
		}
	}

	pub fn signature(&self) -> Vec<u8> {
		match self {
			ProposalType::RefreshProposal { .. } => Vec::new(),
			ProposalType::EVMUnsigned { .. } => Vec::new(),
			ProposalType::EVMSigned { signature, .. } => signature.clone(),
			ProposalType::AnchorUpdate { .. } => Vec::new(),
			ProposalType::AnchorUpdateSigned { signature, .. } => signature.clone(),
			ProposalType::TokenAdd { .. } => Vec::new(),
			ProposalType::TokenAddSigned { signature, .. } => signature.clone(),
			ProposalType::TokenRemove { .. } => Vec::new(),
			ProposalType::TokenRemoveSigned { signature, .. } => signature.clone(),
			ProposalType::WrappingFeeUpdate { .. } => Vec::new(),
			ProposalType::WrappingFeeUpdateSigned { signature, .. } => signature.clone(),
			ProposalType::ResourceIdUpdate { .. } => Vec::new(),
			ProposalType::ResourceIdUpdateSigned { signature, .. } => signature.clone(),
		}
	}
}

pub trait ProposalHandlerTrait {
	fn handle_unsigned_proposal(
		_proposal: Vec<u8>,
		_action: ProposalAction,
	) -> frame_support::pallet_prelude::DispatchResult { Ok(().into()) }

	fn handle_signed_proposal(
		_prop: ProposalType,
		_payload_key: DKGPayloadKey,
	) -> frame_support::pallet_prelude::DispatchResult { Ok(().into()) }

	fn handle_unsigned_refresh_proposal(
		_proposal: RefreshProposal
	) -> frame_support::pallet_prelude::DispatchResult { Ok(().into()) }

	fn handle_signed_refresh_proposal(
		_proposal: RefreshProposal
	) -> frame_support::pallet_prelude::DispatchResult { Ok(().into()) }

	fn handle_evm_signed_proposal(
		_prop: ProposalType,
	) -> frame_support::pallet_prelude::DispatchResult { Ok(().into()) }

	fn handle_anchor_update_signed_proposal(
		_prop: ProposalType,
	) -> frame_support::pallet_prelude::DispatchResult { Ok(().into()) }

	fn handle_token_add_signed_proposal(
		_prop: ProposalType,
	) -> frame_support::pallet_prelude::DispatchResult { Ok(().into()) }

	fn handle_token_remove_signed_proposal(
		_prop: ProposalType,
	) -> frame_support::pallet_prelude::DispatchResult { Ok(().into()) }

	fn handle_wrapping_fee_update_signed_proposal(
		_prop: ProposalType,
	) -> frame_support::pallet_prelude::DispatchResult { Ok(().into()) }

	fn handle_resource_id_update_signed_proposal(
		_prop: ProposalType,
	) -> frame_support::pallet_prelude::DispatchResult { Ok(().into()) }
}

impl ProposalHandlerTrait for () {}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn proposal_encode_decode() {
		let mut proposal_data = Vec::with_capacity(80);
		let chain_id = 5002u32;
		let nonce = 0xffu32;
		let resource_id = {
			let mut r = [0u8; 32];
			r[28..32].copy_from_slice(&chain_id.to_be_bytes());
			r
		};
		let function_sig = [0xaa, 0xbb, 0xcc, 0xdd];
		let proposal_header = ProposalHeader { chain_id, function_sig, nonce, resource_id };
		let src_id = 5001u32;
		let last_leaf_index = nonce as u32;
		let merkle_root = [0xeeu8; 32];
		proposal_header.encode_to(&mut proposal_data);
		proposal_data.extend_from_slice(&src_id.to_be_bytes());
		proposal_data.extend_from_slice(&last_leaf_index.to_be_bytes());
		proposal_data.extend_from_slice(&merkle_root);
		assert_eq!(proposal_data.len(), 80);
		let hex = hex::encode(&proposal_data);
		println!("nonce: {}", nonce);
		println!("src_id: {}", src_id);
		println!("r_id: 0x{}", hex::encode(&resource_id));
		println!("Proposal: 0x{}", hex);

		let decoded_head = ProposalHeader::decode(&mut &proposal_data[..]).unwrap();
		assert_eq!(decoded_head, proposal_header);
	}
}
