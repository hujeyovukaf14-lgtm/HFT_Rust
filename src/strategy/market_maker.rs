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
    pub binance_bid: f64,
    pub binance_ask: f64,
}

impl MarketMaker {
    pub fn new(target_spread: f64) -> Self {
        Self { 
            target_spread,
            tick_counter: 0,
            binance_bid: 0.0,
            binance_ask: 0.0,
        }
    }

    pub fn update_binance_price(&mut self, bid: f64, ask: f64) {
        self.binance_bid = bid;
        self.binance_ask = ask;
        // Optional: Could trigger re-evaluation immediately, but we usually tick on Market Data updates from Bybit too.
        // Or we might want to arbitrage immediately when Binance moves?
        // User request says "update_binance_price(bid, ask). Update on_tick(bybit_book)".
        // Only on_tick returns Action in current signature.
        // We will trigger logic on Bybit ticks for now as per "on_tick" structure, 
        // OR we'll need to call on_tick from Main loop when Binance updates too (passing current Bybit book).
    }

    pub fn on_tick(&mut self, book: &L2OrderBook) -> Option<Action> {
        self.tick_counter += 1;
        
        // 1. Get Best Bid and Ask (Bybit)
        let bybit_bid = book.bids[0];
        let bybit_ask = book.asks[0];

        // Safety check
        if bybit_bid.price == 0.0 || bybit_ask.price == 0.0 {
            return None;
        }

        // Wait for Binance signal
        if self.binance_bid == 0.0 || self.binance_ask == 0.0 {
            return None;
        }

        // 2. Arbitrage Logic (Signal)
        
        // Case A: Binance Bid is significantly higher than Bybit Ask -> Buy on Bybit (undervalued)
        // Spread = Binance_Bid - Bybit_Ask
        let buy_arb_spread = self.binance_bid - bybit_ask.price;
        
        // Case B: Bybit Bid is significantly higher than Binance Ask -> Sell on Bybit (overvalued)
        // Spread = Bybit_Bid - Binance_Ask
        let sell_arb_spread = bybit_bid.price - self.binance_ask;

        if self.tick_counter % 100 == 0 {
             #[cfg(debug_assertions)]
             println!("ARB: Bybit[{:.2}/{:.2}] Binance[{:.2}/{:.2}] Sprd(Buy/Sell): {:.2}/{:.2}", 
                 bybit_bid.price, bybit_ask.price, self.binance_bid, self.binance_ask, buy_arb_spread, sell_arb_spread);
        }

        if buy_arb_spread > self.target_spread {
            // println!("[SIMULATION] ARB SIGNAL: BUY! Diff: {:.2}", buy_arb_spread);
            return Some(Action {
                action_type: ActionType::LimitBuy,
                price: bybit_ask.price, // Markerable Limit (Taket)
                qty: 0.001,
            });
        }

        if sell_arb_spread > self.target_spread {
            // println!("[SIMULATION] ARB SIGNAL: SELL! Diff: {:.2}", sell_arb_spread);
            return Some(Action {
                action_type: ActionType::LimitSell,
                price: bybit_bid.price, // Marketable Limit (Taker)
                qty: 0.001,
            });
        }

        None
    }
}
