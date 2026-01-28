use crate::core::orderbook::L2OrderBook;
use std::time::{Instant, Duration};

#[derive(Debug, Clone, PartialEq)]
pub enum ActionType {
    CreateOrder { price: f64, qty: f64, side: &'static str, link_id: String },
    AmendOrder { price: f64, qty: f64, side: &'static str, link_id: String },
    CancelAll,
    None,
}

#[derive(Debug, Clone)]
pub struct Action {
    pub action_type: ActionType,
}

pub struct MarketMaker {
    target_spread: f64, // Not used for signal now, but maybe for check?
    tick_counter: u64,
    pub binance_bid: f64,
    pub binance_ask: f64,
    
    // State
    last_update_ts: Instant,
    pub has_active_buy: bool,
    pub has_active_sell: bool,
    pub active_buy_link_id: String,
    pub active_sell_link_id: String,
}

impl MarketMaker {
    pub fn new(_target_spread: f64) -> Self {
        Self { 
            target_spread: 0.01,
            tick_counter: 0,
            binance_bid: 0.0,
            binance_ask: 0.0,
            last_update_ts: Instant::now(),
            has_active_buy: false,
            has_active_sell: false,
            active_buy_link_id: "bot-buy-1".to_string(),
            active_sell_link_id: "bot-sell-1".to_string(),
        }
    }

    pub fn update_binance_price(&mut self, bid: f64, ask: f64) {
        self.binance_bid = bid;
        self.binance_ask = ask;
    }

    pub fn on_tick(&mut self, book: &L2OrderBook) -> Option<Vec<Action>> {
        self.tick_counter += 1;
        let mut actions = Vec::new(); // Support multiple actions (Buy + Sell sides)
        
        let bybit_bid = book.bids[0];
        let bybit_ask = book.asks[0];

        if bybit_bid.price == 0.0 || bybit_ask.price == 0.0 { return None; }

        // Rate Limit: 1 Second
        if self.last_update_ts.elapsed() < Duration::from_millis(1000) {
            return None;
        }

        // Logic: 1% away from BBO
        // Target Buy = Bid * 0.99
        // Target Sell = Ask * 1.01
        let target_buy_price = (bybit_bid.price * 0.99 * 100.0).round() / 100.0; // Round to 2 decimals (check tick size?)
        let target_sell_price = (bybit_ask.price * 1.01 * 100.0).round() / 100.0;
        
        // Size: $12 / Price
        let raw_qty: f64 = 12.0 / target_buy_price;
        let buy_qty = raw_qty.max(1.0).round();
        // Assuming RIVER tick size allows... RIVER is typical alt.
        
        // BUY SIDE
        if !self.has_active_buy {
            actions.push(Action {
                action_type: ActionType::CreateOrder {
                    price: target_buy_price,
                    qty: buy_qty,
                    side: "Buy",
                    link_id: self.active_buy_link_id.clone(),
                }
            });
            self.has_active_buy = true; 
        } else {
             actions.push(Action {
                action_type: ActionType::AmendOrder {
                    price: target_buy_price,
                    qty: buy_qty,
                    side: "Buy",
                    link_id: self.active_buy_link_id.clone(),
                }
            });
        }

        // SELL SIDE
        if !self.has_active_sell {
             actions.push(Action {
                action_type: ActionType::CreateOrder {
                    price: target_sell_price,
                    qty: buy_qty, // Sell same size?
                    side: "Sell",
                    link_id: self.active_sell_link_id.clone(),
                }
            });
            self.has_active_sell = true;
        } else {
             actions.push(Action {
                action_type: ActionType::AmendOrder {
                    price: target_sell_price,
                    qty: buy_qty, // Sell same size
                    side: "Sell",
                    link_id: self.active_sell_link_id.clone(),
                }
            });
        }

        self.last_update_ts = Instant::now();
        
        if actions.is_empty() { None } else { Some(actions) }
    }
}
