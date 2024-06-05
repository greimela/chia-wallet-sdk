use chia_protocol::{Coin, CoinSpend};
use chia_puzzles::offer::{NotarizedPayment, SettlementPaymentsSolution};
use chia_sdk_driver::{InnerSpend, SpendContext, SpendError};

#[derive(Debug, Clone)]
#[must_use]
pub struct SettlementSpend {
    notarized_payments: Vec<NotarizedPayment>,
}

impl SettlementSpend {
    pub fn new(notarized_payments: Vec<NotarizedPayment>) -> Self {
        Self { notarized_payments }
    }

    pub fn inner_spend(self, ctx: &mut SpendContext<'_>) -> Result<InnerSpend, SpendError> {
        let puzzle = ctx.settlement_payments_puzzle()?;
        let solution = ctx.alloc(&SettlementPaymentsSolution {
            notarized_payments: self.notarized_payments,
        })?;
        Ok(InnerSpend::new(puzzle, solution))
    }

    pub fn finish(self, ctx: &mut SpendContext<'_>, coin: Coin) -> Result<(), SpendError> {
        let inner_spend = self.inner_spend(ctx)?;
        let puzzle_reveal = ctx.serialize(&inner_spend.puzzle())?;
        let solution = ctx.serialize(&inner_spend.solution())?;
        ctx.spend(CoinSpend::new(coin, puzzle_reveal, solution));
        Ok(())
    }
}
