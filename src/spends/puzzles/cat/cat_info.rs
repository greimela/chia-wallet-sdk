use chia_protocol::{Bytes32, Coin};
use chia_puzzles::LineageProof;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatInfo {
    pub asset_id: Bytes32,
    pub p2_puzzle_hash: Bytes32,
    pub coin: Coin,
    pub lineage_proof: LineageProof,
}