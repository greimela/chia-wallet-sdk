use chia_bls::PublicKey;
use chia_protocol::{Bytes, Bytes32, Coin, CoinSpend, Program};
use chia_wallet::{
    nft::{
        NftIntermediateLauncherArgs, NftOwnershipLayerArgs, NftOwnershipLayerSolution,
        NftRoyaltyTransferPuzzleArgs, NftStateLayerArgs, NftStateLayerSolution,
        NFT_METADATA_UPDATER_PUZZLE_HASH, NFT_OWNERSHIP_LAYER_PUZZLE_HASH,
        NFT_STATE_LAYER_PUZZLE_HASH,
    },
    singleton::{
        LauncherSolution, SingletonArgs, SingletonSolution, SingletonStruct,
        SINGLETON_LAUNCHER_PUZZLE_HASH, SINGLETON_TOP_LAYER_PUZZLE_HASH,
    },
    standard::{StandardArgs, StandardSolution},
    EveProof, Proof,
};
use clvm_traits::{clvm_list, clvm_quote, ToClvm};
use clvm_utils::CurriedProgram;
use clvmr::{
    sha2::{Digest, Sha256},
    NodePtr,
};

use crate::{
    trim_leading_zeros, AssertCoinAnnouncement, AssertPuzzleAnnouncement, CreateCoinWithMemos,
    CreateCoinWithoutMemos, CreatePuzzleAnnouncement, NewNftOwner, SpendContext, SpendError,
};

/// Spend an NFT.
pub fn spend_nft<T>(
    ctx: &mut SpendContext,
    coin: Coin,
    puzzle_reveal: Program,
    proof: Proof,
    conditions: T,
) -> Result<CoinSpend, SpendError>
where
    T: ToClvm<NodePtr>,
{
    // Construct the p2 solution.
    let p2_solution = StandardSolution {
        original_public_key: None,
        delegated_puzzle: clvm_quote!(conditions),
        solution: (),
    };

    // Construct the ownership layer solution.
    let ownership_layer_solution = NftOwnershipLayerSolution {
        inner_solution: p2_solution,
    };

    // Construct the state layer solution.
    let state_layer_solution = NftStateLayerSolution {
        inner_solution: ownership_layer_solution,
    };

    // Construct the singleton solution.
    let solution = ctx.serialize(SingletonSolution {
        proof,
        amount: coin.amount,
        inner_solution: state_layer_solution,
    })?;

    // Construct the coin spend.
    let coin_spend = CoinSpend::new(coin, puzzle_reveal, solution);

    Ok(coin_spend)
}

/// The information required to mint an NFT.
pub struct MintInput {
    /// The owner puzzle hash of the newly minted NFT.
    pub owner_puzzle_hash: Bytes32,
    /// The puzzle hash to send royalties to when trading the NFT.
    pub royalty_puzzle_hash: Bytes32,
    /// The percentage royalty to send to the royalty puzzle hash.
    pub royalty_percentage: u16,
    /// The NFT metadata.
    pub metadata: NodePtr,
    /// The parent coin to spend.
    pub parent_coin_id: Bytes32,
    /// The amount of the launcher coin and subsequent NFT coin.
    pub amount: u64,
}

/// The information required to create and spend an NFT bulk mint.
pub struct BulkMint {
    /// The coin spends for the NFT bulk mint.
    pub coin_spends: Vec<CoinSpend>,
    /// The new NFT outputs.
    pub outputs: Vec<MintOutput>,
}

/// The output of a single NFT mint.
pub struct MintOutput {
    /// The conditions that must be output from the parent to make this mint valid.
    pub parent_conditions: Vec<NodePtr>,
    /// The launcher id of the newly minted NFT.
    pub launcher_id: Bytes32,
}

/// Bulk mints a set of NFTs.
pub fn mint_nfts(
    ctx: &mut SpendContext,
    inputs: Vec<MintInput>,
    synthetic_key: PublicKey,
    did_id: Bytes32,
    did_inner_puzzle_hash: Bytes32,
) -> Result<BulkMint, SpendError> {
    let mut coin_spends = Vec::new();
    let mut outputs = Vec::new();

    let standard_puzzle = ctx.standard_puzzle();
    let royalty_transfer_puzzle = ctx.nft_royalty_transfer();
    let ownership_puzzle = ctx.nft_ownership_layer();
    let state_puzzle = ctx.nft_state_layer();
    let singleton_puzzle = ctx.singleton_top_layer();
    let launcher_puzzle = ctx.singleton_launcher();

    let p2 = ctx.alloc(CurriedProgram {
        program: standard_puzzle,
        args: StandardArgs { synthetic_key },
    })?;

    let mint_total = inputs.len();

    for (mint_index, input) in inputs.into_iter().enumerate() {
        let mut parent_conditions = Vec::new();

        // Create the intermediate launcher.
        let intermediate_spend =
            spend_new_intermediate_launcher(ctx, input.parent_coin_id, mint_index, mint_total)?;
        let intermediate_id = intermediate_spend.coin.coin_id();

        parent_conditions.push(ctx.alloc(CreateCoinWithoutMemos {
            puzzle_hash: intermediate_spend.coin.puzzle_hash,
            amount: intermediate_spend.coin.amount,
        })?);

        let mut index_message = Sha256::new();
        index_message.update(usize_to_bytes(mint_index));
        index_message.update(usize_to_bytes(mint_total));

        let mut announcement_id = Sha256::new();
        announcement_id.update(intermediate_id);
        announcement_id.update(index_message.finalize());

        parent_conditions.push(ctx.alloc(AssertCoinAnnouncement {
            announcement_id: Bytes::new(announcement_id.finalize().to_vec()),
        })?);

        coin_spends.push(intermediate_spend);

        // Construct the eve NFT.
        let launcher_coin = Coin::new(
            intermediate_id,
            SINGLETON_LAUNCHER_PUZZLE_HASH.into(),
            input.amount,
        );
        let launcher_id = launcher_coin.coin_id();

        parent_conditions.push(ctx.alloc(CreatePuzzleAnnouncement {
            message: launcher_id.to_vec().into(),
        })?);

        let singleton_struct = SingletonStruct {
            mod_hash: SINGLETON_TOP_LAYER_PUZZLE_HASH.into(),
            launcher_id,
            launcher_puzzle_hash: SINGLETON_LAUNCHER_PUZZLE_HASH.into(),
        };

        let royalty_transfer = CurriedProgram {
            program: royalty_transfer_puzzle,
            args: NftRoyaltyTransferPuzzleArgs {
                singleton_struct: singleton_struct.clone(),
                royalty_puzzle_hash: input.royalty_puzzle_hash,
                trade_price_percentage: input.royalty_percentage,
            },
        };

        let ownership_layer = CurriedProgram {
            program: ownership_puzzle,
            args: NftOwnershipLayerArgs {
                mod_hash: NFT_OWNERSHIP_LAYER_PUZZLE_HASH.into(),
                current_owner: None,
                transfer_program: royalty_transfer,
                inner_puzzle: p2,
            },
        };

        let state_layer = CurriedProgram {
            program: state_puzzle,
            args: NftStateLayerArgs {
                mod_hash: NFT_STATE_LAYER_PUZZLE_HASH.into(),
                metadata: input.metadata,
                metadata_updater_puzzle_hash: NFT_METADATA_UPDATER_PUZZLE_HASH.into(),
                inner_puzzle: ownership_layer,
            },
        };

        let singleton = ctx.alloc(CurriedProgram {
            program: singleton_puzzle,
            args: SingletonArgs {
                singleton_struct,
                inner_puzzle: state_layer,
            },
        })?;

        let eve_puzzle_hash = ctx.tree_hash(singleton);

        let eve_message = ctx.alloc(clvm_list!(eve_puzzle_hash, input.amount, ()))?;
        let eve_message_hash = ctx.tree_hash(eve_message);

        let mut announcement_id = Sha256::new();
        announcement_id.update(launcher_id);
        announcement_id.update(eve_message_hash);

        parent_conditions.push(ctx.alloc(AssertCoinAnnouncement {
            announcement_id: Bytes::new(announcement_id.finalize().to_vec()),
        })?);

        // Spend the launcher coin.
        let launcher_puzzle_reveal = ctx.serialize(launcher_puzzle)?;
        let launcher_solution = ctx.serialize(LauncherSolution {
            singleton_puzzle_hash: eve_puzzle_hash,
            amount: input.amount,
            key_value_list: (),
        })?;

        coin_spends.push(CoinSpend::new(
            launcher_coin,
            launcher_puzzle_reveal,
            launcher_solution,
        ));

        // Spend the eve coin.
        let eve_coin = Coin::new(launcher_id, eve_puzzle_hash, input.amount);

        let eve_proof = Proof::Eve(EveProof {
            parent_coin_info: intermediate_id,
            amount: input.amount,
        });

        let eve_puzzle_reveal = ctx.serialize(singleton)?;

        let eve_coin_spend = spend_nft(
            ctx,
            eve_coin,
            eve_puzzle_reveal,
            eve_proof,
            clvm_list!(
                CreateCoinWithMemos {
                    puzzle_hash: input.owner_puzzle_hash,
                    amount: input.amount,
                    memos: vec![Bytes::new(input.owner_puzzle_hash.to_vec())],
                },
                NewNftOwner {
                    new_owner: Some(did_id),
                    trade_prices_list: Vec::new(),
                    new_did_inner_hash: Some(did_inner_puzzle_hash)
                }
            ),
        )?;
        let new_nft_owner_args = ctx.alloc(clvm_list!(did_id, (), did_inner_puzzle_hash))?;

        coin_spends.push(eve_coin_spend);

        let mut announcement_id = Sha256::new();
        announcement_id.update(eve_puzzle_hash);
        announcement_id.update([0xad, 0x4c]);
        announcement_id.update(ctx.tree_hash(new_nft_owner_args));

        parent_conditions.push(ctx.alloc(AssertPuzzleAnnouncement {
            announcement_id: Bytes::new(announcement_id.finalize().to_vec()),
        })?);

        // Finalize the output.
        outputs.push(MintOutput {
            parent_conditions,
            launcher_id,
        });
    }

    Ok(BulkMint {
        coin_spends,
        outputs,
    })
}

fn spend_new_intermediate_launcher(
    ctx: &mut SpendContext,
    parent_coin_id: Bytes32,
    index: usize,
    total: usize,
) -> Result<CoinSpend, SpendError> {
    let intermediate_puzzle = ctx.nft_intermediate_launcher();

    let puzzle = ctx.alloc(CurriedProgram {
        program: intermediate_puzzle,
        args: NftIntermediateLauncherArgs {
            launcher_puzzle_hash: SINGLETON_LAUNCHER_PUZZLE_HASH.into(),
            mint_number: index,
            mint_total: total,
        },
    })?;
    let puzzle_reveal = ctx.serialize(puzzle)?;
    let solution = ctx.serialize(())?;

    let puzzle_hash = ctx.tree_hash(puzzle);

    Ok(CoinSpend::new(
        Coin::new(parent_coin_id, puzzle_hash, 0),
        puzzle_reveal,
        solution,
    ))
}

fn usize_to_bytes(amount: usize) -> Vec<u8> {
    let bytes: Vec<u8> = amount.to_be_bytes().into();
    trim_leading_zeros(bytes.as_slice()).to_vec()
}