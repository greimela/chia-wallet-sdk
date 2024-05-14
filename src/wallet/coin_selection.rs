use std::{cmp::Reverse, collections::HashSet};

use chia_protocol::Coin;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use thiserror::Error;

/// An error that occurs when selecting coins.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CoinSelectionError {
    /// There were no spendable coins to select from.
    #[error("no spendable coins")]
    NoSpendableCoins,

    /// There weren't enough coins to reach the amount.
    #[error("insufficient balance {0}")]
    InsufficientBalance(u128),

    /// The selected coins exceeded the maximum.
    #[error("exceeded max coins")]
    ExceededMaxCoins,
}

/// Uses the knapsack algorithm to select coins.
pub fn select_coins(
    mut spendable_coins: Vec<Coin>,
    amount: u128,
) -> Result<HashSet<Coin>, CoinSelectionError> {
    let max_coins = 500;

    // You cannot spend no coins.
    if spendable_coins.is_empty() {
        return Err(CoinSelectionError::NoSpendableCoins);
    }

    // Checks to ensure the balance is sufficient before continuing.
    let spendable_amount = spendable_coins
        .iter()
        .fold(0u128, |acc, coin| acc + coin.amount as u128);

    if spendable_amount < amount {
        return Err(CoinSelectionError::InsufficientBalance(spendable_amount));
    }

    // Sorts by amount, descending.
    spendable_coins.sort_unstable_by_key(|coin| Reverse(coin.amount));

    // Exact coin match.
    for coin in spendable_coins.iter() {
        if coin.amount as u128 == amount {
            let mut result = HashSet::new();
            result.insert(*coin);
            return Ok(result);
        }
    }

    let mut smaller_coins = HashSet::new();
    let mut smaller_sum = 0;

    for coin in spendable_coins.iter() {
        let coin_amount = coin.amount as u128;

        if coin_amount < amount {
            smaller_coins.insert(*coin);
            smaller_sum += coin_amount;
        }
    }

    // Check for an exact match.
    if smaller_sum == amount && smaller_coins.len() < max_coins && amount != 0 {
        return Ok(smaller_coins);
    }

    // There must be a single coin larger than the amount.
    if smaller_sum < amount {
        let smallest_coin = smallest_coin_above(&spendable_coins, amount).unwrap();
        let mut result = HashSet::new();
        result.insert(smallest_coin);
        return Ok(result);
    }

    // Apply the knapsack algorithm otherwise.
    if smaller_sum > amount {
        if let Some(result) = knapsack_coin_algorithm(
            &mut ChaCha8Rng::seed_from_u64(0),
            &spendable_coins,
            amount,
            u128::MAX,
            max_coins,
        ) {
            return Ok(result);
        }

        // Knapsack failed to select coins, so try summing the largest coins.
        let summed_coins = sum_largest_coins(&spendable_coins, amount);

        if summed_coins.len() <= max_coins {
            return Ok(summed_coins);
        } else {
            return Err(CoinSelectionError::ExceededMaxCoins);
        }
    }

    // Try to find a large coin to select.
    if let Some(coin) = smallest_coin_above(&spendable_coins, amount) {
        let mut result = HashSet::new();
        result.insert(coin);
        return Ok(result);
    }

    // It would require too many coins to match the amount.
    Err(CoinSelectionError::ExceededMaxCoins)
}

fn sum_largest_coins(coins: &[Coin], amount: u128) -> HashSet<Coin> {
    let mut selected_coins = HashSet::new();
    let mut selected_sum = 0;
    for coin in coins {
        selected_sum += coin.amount as u128;
        selected_coins.insert(*coin);

        if selected_sum >= amount {
            return selected_coins;
        }
    }
    unreachable!()
}

fn smallest_coin_above(coins: &[Coin], amount: u128) -> Option<Coin> {
    if (coins[0].amount as u128) < amount {
        return None;
    }
    for coin in coins.iter().rev() {
        if (coin.amount as u128) >= amount {
            return Some(*coin);
        }
    }
    unreachable!();
}

/// Runs the knapsack algorithm on a set of coins, attempting to find an optimal set.
pub fn knapsack_coin_algorithm(
    rng: &mut impl Rng,
    spendable_coins: &[Coin],
    amount: u128,
    max_amount: u128,
    max_coins: usize,
) -> Option<HashSet<Coin>> {
    let mut best_sum = max_amount;
    let mut best_coins = None;

    for _ in 0..1000 {
        let mut selected_coins = HashSet::new();
        let mut selected_sum = 0;
        let mut target_reached = false;

        for pass in 0..2 {
            if target_reached {
                break;
            }

            for coin in spendable_coins {
                let filter_first = pass != 0 || !rng.gen::<bool>();
                let filter_second = pass != 1 || selected_coins.contains(coin);

                if filter_first && filter_second {
                    continue;
                }

                if selected_coins.len() > max_coins {
                    break;
                }

                selected_sum += coin.amount as u128;
                selected_coins.insert(*coin);

                if selected_sum == amount {
                    return Some(selected_coins);
                }

                if selected_sum > amount {
                    target_reached = true;

                    if selected_sum < best_sum {
                        best_sum = selected_sum;
                        best_coins = Some(selected_coins.clone());

                        selected_sum -= coin.amount as u128;
                        selected_coins.remove(coin);
                    }
                }
            }
        }
    }

    best_coins
}

#[cfg(test)]
mod tests {
    use chia_protocol::Bytes32;

    use super::*;

    macro_rules! coin_list {
        ( $( $coin:expr ),* $(,)? ) => {
            vec![$( coin($coin) ),*]
        };
    }

    macro_rules! coin_set {
        ( $( $coin:expr ),* $(,)? ) => {{
            let mut coins = HashSet::new();
            $( coins.insert(coin($coin)); )*
            coins
        }};
    }

    fn coin(amount: u64) -> Coin {
        Coin::new(Bytes32::from([0; 32]), Bytes32::from([0; 32]), amount)
    }

    #[test]
    fn test_select_coins() {
        let coins = coin_list![100, 200, 300, 400, 500];

        // Sorted by amount, ascending.
        let selected = select_coins(coins, 700).unwrap();
        let expected = coin_set![300, 400];
        assert_eq!(selected, expected);
    }

    #[test]
    fn test_insufficient_balance() {
        let coins = coin_list![50, 250, 100000];

        // Select an amount that is too high.
        let selected = select_coins(coins, 9999999);
        assert_eq!(
            selected,
            Err(CoinSelectionError::InsufficientBalance(100300))
        );
    }

    #[test]
    fn test_no_coins() {
        // There is no amount to select from.
        let selected = select_coins(Vec::new(), 100);
        assert_eq!(selected, Err(CoinSelectionError::NoSpendableCoins));

        // Even if the amount is zero, this should fail.
        let selected = select_coins(Vec::new(), 0);
        assert_eq!(selected, Err(CoinSelectionError::NoSpendableCoins));
    }
}
