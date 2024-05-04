use chia_bls::PublicKey;
use chia_protocol::{Bytes32, Coin};
use chia_wallet::{
    nft::{
        NFT_METADATA_UPDATER_PUZZLE_HASH, NFT_OWNERSHIP_LAYER_PUZZLE_HASH,
        NFT_ROYALTY_TRANSFER_PUZZLE_HASH, NFT_STATE_LAYER_PUZZLE_HASH,
    },
    singleton::{SINGLETON_LAUNCHER_PUZZLE_HASH, SINGLETON_TOP_LAYER_PUZZLE_HASH},
    standard::standard_puzzle_hash,
    EveProof, LineageProof, Proof,
};
use clvm_traits::{clvm_list, ToClvm};
use clvm_utils::{curry_tree_hash, tree_hash_atom, tree_hash_pair};
use clvmr::NodePtr;
use sha2::{Digest, Sha256};

use crate::{
    spend_nft, u16_to_bytes, AssertPuzzleAnnouncement, ChainedSpend, CreateCoinWithMemos,
    LaunchSingleton, NewNftOwner, NftInfo, SpendContext, SpendError, StandardSpend,
};

pub trait MintNft {
    fn mint_eve_nft<M>(
        self,
        ctx: &mut SpendContext,
        inner_puzzle_hash: Bytes32,
        metadata: M,
        metadata_updater_hash: Bytes32,
        royalty_puzzle_hash: Bytes32,
        royalty_percentage: u16,
    ) -> Result<(ChainedSpend, Bytes32, NftInfo<M>), SpendError>
    where
        M: ToClvm<NodePtr>;

    fn mint_custom_standard_nft<M>(
        self,
        ctx: &mut SpendContext,
        metadata: M,
        metadata_updater_hash: Bytes32,
        royalty_puzzle_hash: Bytes32,
        royalty_percentage: u16,
        synthetic_key: PublicKey,
        did_id: Bytes32,
        did_inner_puzzle_hash: Bytes32,
    ) -> Result<(ChainedSpend, NftInfo<M>), SpendError>
    where
        M: ToClvm<NodePtr>,
        Self: Sized,
    {
        let inner_puzzle_hash = standard_puzzle_hash(&synthetic_key).into();

        let (mut mint_nft, nft_inner_puzzle_hash, mut nft_info) = self.mint_eve_nft(
            ctx,
            inner_puzzle_hash,
            metadata,
            metadata_updater_hash,
            royalty_puzzle_hash,
            royalty_percentage,
        )?;

        let (inner_spend, _) = StandardSpend::new()
            .condition(ctx.alloc(CreateCoinWithMemos {
                puzzle_hash: inner_puzzle_hash,
                amount: nft_info.coin.amount,
                memos: vec![inner_puzzle_hash.to_vec().into()],
            })?)
            .condition(ctx.alloc(NewNftOwner {
                new_owner: Some(did_id),
                trade_prices_list: Vec::new(),
                new_did_inner_hash: Some(did_inner_puzzle_hash),
            })?)
            .inner_spend(ctx, synthetic_key)?;

        let new_nft_owner_args = ctx.alloc(clvm_list!(did_id, (), did_inner_puzzle_hash))?;

        let mut announcement_id = Sha256::new();
        announcement_id.update(nft_info.coin.puzzle_hash);
        announcement_id.update([0xad, 0x4c]);
        announcement_id.update(ctx.tree_hash(new_nft_owner_args));

        mint_nft
            .parent_conditions
            .push(ctx.alloc(AssertPuzzleAnnouncement {
                announcement_id: Bytes32::new(announcement_id.finalize().into()),
            })?);

        let spend = spend_nft(ctx, &nft_info, inner_spend)?;
        mint_nft.coin_spends.push(spend);

        nft_info.proof = Proof::Lineage(LineageProof {
            parent_coin_info: nft_info.launcher_id,
            inner_puzzle_hash: nft_inner_puzzle_hash,
            amount: nft_info.coin.amount,
        });

        nft_info.coin = Coin::new(
            nft_info.coin.coin_id(),
            nft_info.coin.puzzle_hash,
            nft_info.coin.amount,
        );

        Ok((mint_nft, nft_info))
    }

    fn mint_standard_nft<M>(
        self,
        ctx: &mut SpendContext,
        metadata: M,
        royalty_percentage: u16,
        synthetic_key: PublicKey,
        did_id: Bytes32,
        did_inner_puzzle_hash: Bytes32,
    ) -> Result<(ChainedSpend, NftInfo<M>), SpendError>
    where
        M: ToClvm<NodePtr>,
        Self: Sized,
    {
        let royalty_puzzle_hash = standard_puzzle_hash(&synthetic_key);

        self.mint_custom_standard_nft(
            ctx,
            metadata,
            NFT_METADATA_UPDATER_PUZZLE_HASH.into(),
            royalty_puzzle_hash.into(),
            royalty_percentage,
            synthetic_key,
            did_id,
            did_inner_puzzle_hash,
        )
    }
}

impl MintNft for LaunchSingleton {
    fn mint_eve_nft<M>(
        self,
        ctx: &mut SpendContext,
        inner_puzzle_hash: Bytes32,
        metadata: M,
        metadata_updater_hash: Bytes32,
        royalty_puzzle_hash: Bytes32,
        royalty_percentage: u16,
    ) -> Result<(ChainedSpend, Bytes32, NftInfo<M>), SpendError>
    where
        M: ToClvm<NodePtr>,
    {
        let metadata_ptr = ctx.alloc(&metadata)?;
        let metadata_hash = ctx.tree_hash(metadata_ptr);

        let ownership_layer_hash = nft_ownership_layer_hash(
            None,
            nft_royalty_transfer_hash(
                self.coin().coin_id(),
                royalty_puzzle_hash,
                royalty_percentage,
            ),
            inner_puzzle_hash,
        );
        let nft_inner_puzzle_hash =
            nft_state_layer_hash(metadata_hash, metadata_updater_hash, ownership_layer_hash);

        let launcher_coin = self.coin().clone();
        let (chained_spend, eve_coin) = self.finish(ctx, nft_inner_puzzle_hash, ())?;

        let proof = Proof::Eve(EveProof {
            parent_coin_info: launcher_coin.parent_coin_info,
            amount: launcher_coin.amount,
        });

        let nft_info = NftInfo {
            launcher_id: launcher_coin.coin_id(),
            coin: eve_coin,
            proof,
            metadata,
            metadata_updater_hash,
            current_owner: None,
            royalty_puzzle_hash,
            royalty_percentage,
        };

        Ok((chained_spend, nft_inner_puzzle_hash, nft_info))
    }
}

pub fn nft_state_layer_hash(
    metadata_hash: Bytes32,
    metadata_updater_hash: Bytes32,
    inner_puzzle_hash: Bytes32,
) -> Bytes32 {
    let mod_hash = tree_hash_atom(&NFT_STATE_LAYER_PUZZLE_HASH);
    let metadata_updater_hash = tree_hash_atom(&metadata_updater_hash);

    curry_tree_hash(
        NFT_STATE_LAYER_PUZZLE_HASH,
        &[
            mod_hash,
            metadata_hash.into(),
            metadata_updater_hash,
            inner_puzzle_hash.into(),
        ],
    )
    .into()
}

pub fn nft_ownership_layer_hash(
    current_owner: Option<Bytes32>,
    transfer_program_hash: Bytes32,
    inner_puzzle_hash: Bytes32,
) -> Bytes32 {
    let mod_hash = tree_hash_atom(&NFT_OWNERSHIP_LAYER_PUZZLE_HASH);
    let current_owner_hash = match current_owner {
        Some(did_id) => tree_hash_atom(&did_id),
        None => tree_hash_atom(&[]),
    };

    curry_tree_hash(
        NFT_OWNERSHIP_LAYER_PUZZLE_HASH,
        &[
            mod_hash,
            current_owner_hash,
            transfer_program_hash.into(),
            inner_puzzle_hash.into(),
        ],
    )
    .into()
}

pub fn nft_royalty_transfer_hash(
    launcher_id: Bytes32,
    royalty_puzzle_hash: Bytes32,
    royalty_percentage: u16,
) -> Bytes32 {
    let royalty_puzzle_hash = tree_hash_atom(&royalty_puzzle_hash);
    let royalty_percentage_hash = tree_hash_atom(&u16_to_bytes(royalty_percentage));

    let singleton_hash = tree_hash_atom(&SINGLETON_TOP_LAYER_PUZZLE_HASH);
    let launcher_id_hash = tree_hash_atom(&launcher_id);
    let launcher_puzzle_hash = tree_hash_atom(&SINGLETON_LAUNCHER_PUZZLE_HASH);

    let pair = tree_hash_pair(launcher_id_hash, launcher_puzzle_hash);
    let singleton_struct_hash = tree_hash_pair(singleton_hash, pair);

    curry_tree_hash(
        NFT_ROYALTY_TRANSFER_PUZZLE_HASH,
        &[
            singleton_struct_hash,
            royalty_puzzle_hash,
            royalty_percentage_hash,
        ],
    )
    .into()
}

#[cfg(test)]
mod tests {
    use chia_bls::{sign, Signature};
    use chia_protocol::SpendBundle;
    use chia_wallet::{
        nft::{
            NftOwnershipLayerArgs, NftRoyaltyTransferPuzzleArgs, NftStateLayerArgs,
            NFT_METADATA_UPDATER_PUZZLE_HASH, NFT_OWNERSHIP_LAYER_PUZZLE_HASH,
            NFT_STATE_LAYER_PUZZLE_HASH,
        },
        singleton::{
            SingletonStruct, SINGLETON_LAUNCHER_PUZZLE_HASH, SINGLETON_TOP_LAYER_PUZZLE_HASH,
        },
        standard::DEFAULT_HIDDEN_PUZZLE_HASH,
        DeriveSynthetic,
    };
    use clvm_utils::CurriedProgram;
    use clvmr::Allocator;

    use crate::{
        intermediate_launcher, spend_did, testing::SECRET_KEY, CreateDid, RequiredSignature,
        WalletSimulator,
    };

    use super::*;

    #[tokio::test]
    async fn test_bulk_mint() -> anyhow::Result<()> {
        let sim = WalletSimulator::new().await;
        let peer = sim.peer().await;

        let mut allocator = Allocator::new();
        let mut ctx = SpendContext::new(&mut allocator);

        let sk = SECRET_KEY.derive_synthetic(&DEFAULT_HIDDEN_PUZZLE_HASH);
        let pk = sk.public_key();

        let puzzle_hash = Bytes32::new(standard_puzzle_hash(&pk));

        let parent = sim.generate_coin(puzzle_hash, 3).await.coin;

        let (create_did, did_info) =
            LaunchSingleton::new(parent.coin_id(), 1).create_standard_did(&mut ctx, pk.clone())?;

        let mut coin_spends =
            StandardSpend::new()
                .chain(create_did)
                .finish(&mut ctx, parent, pk.clone())?;

        let (intermediate, launcher) =
            intermediate_launcher(&mut ctx, did_info.coin.coin_id(), 0, 1)?;

        let (nft_mint, _nft_info) = launcher.mint_standard_nft(
            &mut ctx,
            (),
            100,
            pk.clone(),
            did_info.launcher_id,
            did_info.did_inner_puzzle_hash,
        )?;

        let (inner_spend, did_spends) = StandardSpend::new()
            .chain(intermediate)
            .chain(nft_mint)
            .inner_spend(&mut ctx, pk)?;

        coin_spends.extend(did_spends);
        coin_spends.push(spend_did(&mut ctx, &did_info, inner_spend)?);

        let required_signatures = RequiredSignature::from_coin_spends(
            &mut allocator,
            &coin_spends,
            WalletSimulator::AGG_SIG_ME.into(),
        )?;

        let mut aggregated_signature = Signature::default();

        for required in required_signatures {
            aggregated_signature += &sign(&sk, required.final_message());
        }

        let spend_bundle = SpendBundle::new(coin_spends, aggregated_signature);
        let ack = peer.send_transaction(spend_bundle).await?;

        assert_eq!(ack.error, None);
        assert_eq!(ack.status, 1);

        Ok(())
    }

    #[test]
    fn test_state_layer_hash() {
        let mut allocator = Allocator::new();
        let mut ctx = SpendContext::new(&mut allocator);

        let inner_puzzle = ctx.alloc([1, 2, 3]).unwrap();
        let inner_puzzle_hash = ctx.tree_hash(inner_puzzle);

        let metadata = ctx.alloc([4, 5, 6]).unwrap();
        let metadata_hash = ctx.tree_hash(metadata);

        let nft_state_layer = ctx.nft_state_layer();

        let puzzle = ctx
            .alloc(CurriedProgram {
                program: nft_state_layer,
                args: NftStateLayerArgs {
                    mod_hash: NFT_STATE_LAYER_PUZZLE_HASH.into(),
                    metadata,
                    metadata_updater_puzzle_hash: NFT_METADATA_UPDATER_PUZZLE_HASH.into(),
                    inner_puzzle,
                },
            })
            .unwrap();
        let allocated_puzzle_hash = ctx.tree_hash(puzzle);

        let puzzle_hash = nft_state_layer_hash(
            metadata_hash,
            NFT_METADATA_UPDATER_PUZZLE_HASH.into(),
            inner_puzzle_hash,
        );

        assert_eq!(hex::encode(allocated_puzzle_hash), hex::encode(puzzle_hash));
    }

    #[test]
    fn test_ownership_layer_hash() {
        let mut allocator = Allocator::new();
        let mut ctx = SpendContext::new(&mut allocator);

        let inner_puzzle = ctx.alloc([1, 2, 3]).unwrap();
        let inner_puzzle_hash = ctx.tree_hash(inner_puzzle);

        let launcher_id = Bytes32::new([69; 32]);

        let royalty_puzzle_hash = Bytes32::new([34; 32]);
        let royalty_percentage = 100;

        let current_owner = Some(Bytes32::new([42; 32]));

        let nft_ownership_layer = ctx.nft_ownership_layer();
        let nft_royalty_transfer = ctx.nft_royalty_transfer();

        let transfer_program = ctx
            .alloc(CurriedProgram {
                program: nft_royalty_transfer,
                args: NftRoyaltyTransferPuzzleArgs {
                    singleton_struct: SingletonStruct {
                        mod_hash: SINGLETON_TOP_LAYER_PUZZLE_HASH.into(),
                        launcher_id,
                        launcher_puzzle_hash: SINGLETON_LAUNCHER_PUZZLE_HASH.into(),
                    },
                    royalty_puzzle_hash,
                    trade_price_percentage: royalty_percentage,
                },
            })
            .unwrap();
        let allocated_transfer_program_hash = ctx.tree_hash(transfer_program);

        let puzzle = ctx
            .alloc(CurriedProgram {
                program: nft_ownership_layer,
                args: NftOwnershipLayerArgs {
                    mod_hash: NFT_OWNERSHIP_LAYER_PUZZLE_HASH.into(),
                    current_owner,
                    transfer_program,
                    inner_puzzle,
                },
            })
            .unwrap();
        let allocated_puzzle_hash = ctx.tree_hash(puzzle);

        let puzzle_hash = nft_ownership_layer_hash(
            current_owner,
            allocated_transfer_program_hash,
            inner_puzzle_hash,
        );

        let transfer_program_hash =
            nft_royalty_transfer_hash(launcher_id, royalty_puzzle_hash, royalty_percentage);

        assert_eq!(
            hex::encode(allocated_transfer_program_hash),
            hex::encode(transfer_program_hash)
        );

        assert_eq!(hex::encode(allocated_puzzle_hash), hex::encode(puzzle_hash));
    }
}
