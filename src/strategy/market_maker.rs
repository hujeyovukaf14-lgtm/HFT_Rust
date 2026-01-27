use crate::core::orderbook::L2OrderBook;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActionType {
    LimitBuy,
    LimitSell,
    CancelAll,
}

#[derive(Debug, Clone, Copy)]
pub struct Action {
    pub action_type: ActionType,
    pub price: f64,
    pub qty: f64,
}

pub struct MarketMaker {
    target_spread: f64,
    tick_counter: u64,
}

impl MarketMaker {
    pub fn new(target_spread: f64) -> Self {
        Self { 
            target_spread,
            tick_counter: 0 
        }
    }

    pub fn on_tick(&mut self, book: &L2OrderBook) -> Option<Action> {
        self.tick_counter += 1;
        
        // 1. Get Best Bid and Ask
        // Assuming book is sorted (0 is best)
        let best_bid = book.bids[0];
        let best_ask = book.asks[0];

        // Safety check: Empty book
        if best_bid.price == 0.0 || best_ask.price == 0.0 {
            return None;
        }

        let spread = best_ask.price - best_bid.price;

        // Log first 10, then every 100
        if self.tick_counter <= 10 || self.tick_counter % 100 == 0 {
             #[cfg(debug_assertions)]
             println!("MARKET: Best Bid = {:.2}, Best Ask = {:.2}, Spread = {:.2}", 
                 best_bid.price, best_ask.price, spread);
        }

        // 2. Simple Logic
        if spread > self.target_spread {
            // "Make" the market: Buy at Best Bid + epsilon, Sell at Best Ask - epsilon?
            // For now, just simplistic "Join Best Bid" logic to prove pipeline
            
            // Example: If spread is huge, place a bid
            return Some(Action {
                action_type: ActionType::LimitBuy,
                price: best_bid.price, // Join the bid
                qty: 0.001,
            });
        }

        None
    }
}
