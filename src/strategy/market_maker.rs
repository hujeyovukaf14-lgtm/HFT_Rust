use crate::core::orderbook::L2OrderBook;
use std::time::{Instant, Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq)]
pub enum ActionType {
    CreateOrder { price: f64, qty: f64, side: &'static str, link_id: String },
    AmendOrder { price: f64, qty: f64, side: &'static str, link_id: String },
    CancelOrder { link_id: String },
    ClosePosition { qty: f64, side: &'static str },
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
    pub position: f64,
    pub entry_price: f64,
    pub last_trade_ts: Option<Instant>,
    pub active_buy_price: f64,
    pub active_sell_price: f64,
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

            active_buy_link_id: format!("b-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()),
            active_sell_link_id: format!("s-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()),
            position: 0.0,
            entry_price: 0.0,
            last_trade_ts: None,
            active_buy_price: 0.0,
            active_sell_price: 0.0,
        }
    }

    pub fn update_binance_price(&mut self, bid: f64, ask: f64) {
        self.binance_bid = bid;
        self.binance_ask = ask;
    }
    
    pub fn on_fill(&mut self, side: &str, qty: f64, px: f64) {
        // Weighted Average Entry Price
        if self.position == 0.0 {
            self.entry_price = px;
        } else {
             // If adding to position (same side)
             let is_long = self.position > 0.0;
             let is_buy = side == "Buy";
             if (is_long && is_buy) || (!is_long && !is_buy) {
                 let total_val = (self.position.abs() * self.entry_price) + (qty * px);
                 let new_qty = self.position.abs() + qty;
                 self.entry_price = total_val / new_qty;
             }
             // If reducing, entry price stays same, realized PnL happens.
        }

        if side == "Buy" {
            self.position += qty;
        } else {
            self.position -= qty;
        }
        
        if self.position.abs() < 0.0001 {
             self.entry_price = 0.0;
        }

        self.last_trade_ts = Some(Instant::now());
        println!("STRATEGY: Fill detected! Side: {}, Qty: {}, Px: {}, New Pos: {}, AvgEntry: {}", side, qty, px, self.position, self.entry_price);
    }

    pub fn on_order_cancel(&mut self, side: &str) {
        if side == "Buy" {
            self.has_active_buy = false;
            self.active_buy_price = 0.0;
        } else if side == "Sell" {
            self.has_active_sell = false;
            self.active_sell_price = 0.0;
        }
    }
    
    pub fn sync_position(&mut self, user_position: f64, avg_price: f64) {
        // Only update if significantly different to avoid fighting with on_fill
        if (self.position - user_position).abs() > 0.0001 {
            println!("STRATEGY: Syncing Position State! Old: {}, New: {}", self.position, user_position);
            self.position = user_position;
            self.entry_price = avg_price;
            
            // If we suddenly have a position and didn't before, start the timer?
            // Or if we are just syncing, maybe we shouldn't reset timer if it's already running?
            if self.position.abs() > 0.0001 && self.last_trade_ts.is_none() {
                self.last_trade_ts = Some(Instant::now());
            }
            // If position closed externally
            if self.position.abs() < 0.0001 {
                self.last_trade_ts = None;
                self.entry_price = 0.0;
                // DO NOT cancel orders here aggressively, on_tick will handle cancellations if needed
            }
        }
    }

    pub fn on_tick(&mut self, book: &L2OrderBook) -> Option<Vec<Action>> {
        self.tick_counter += 1;
        let mut actions = Vec::new(); // Support multiple actions (Buy + Sell sides)

        // 0. CLOSE POSITION LOGIC (Scalp)
        if self.position.abs() > 0.0001 { // Float epsilon
             let current_bid = book.bids[0].price;
             let current_ask = book.asks[0].price;

             let mut close_signal = false;
             let mut reason = "";
             
             // C. Calc PnL for logic
             let unrealized_pnl = if self.position > 0.0 {
                 (current_bid - self.entry_price) / self.entry_price
             } else {
                 (self.entry_price - current_ask) / self.entry_price
             };

             // A. Time-based Exit (3 seconds) - ONLY IF NOT IN PROFIT
             // If we are profitable, we hold (let it run to TP). If losing, we kill it quickly.
             if unrealized_pnl <= 0.0 {
                 if let Some(ts) = self.last_trade_ts {
                     if ts.elapsed() > Duration::from_secs(3) {
                         close_signal = true;
                         reason = "Time Limit (3s) & Loss";
                     }
                 }
             }

             // B. Take Profit (0.3%)
             // Long: Sell > Entry * 1.003
             // Short: Buy < Entry * 0.997
             // Logic remains same
             if self.position > 0.0 {
                 if current_bid > self.entry_price * 1.003 {
                     close_signal = true;
                     reason = "Take Profit (+0.3%)";
                 }
             } else {
                 if current_ask < self.entry_price * 0.997 {
                     close_signal = true; 
                     reason = "Take Profit (+0.3%)";
                 }
             }
             
             if close_signal {
                 println!("STRATEGY: Closing Position! Reason: {} | Pos: {} | Entry: {}", reason, self.position, self.entry_price);
                 
                 // 1. Cancel Active Orders first to free up margin/inventory
                 if self.has_active_buy {
                     actions.push(Action {
                         action_type: ActionType::CancelOrder { 
                             link_id: self.active_buy_link_id.clone() 
                         }
                     });
                     self.has_active_buy = false;
                 }
                 if self.has_active_sell {
                     actions.push(Action {
                         action_type: ActionType::CancelOrder { 
                             link_id: self.active_sell_link_id.clone() 
                         }
                     });
                     self.has_active_sell = false;
                 }

                 let close_side = if self.position > 0.0 { "Sell" } else { "Buy" };
                 actions.push(Action {
                     action_type: ActionType::ClosePosition {
                         qty: self.position.abs(),
                         side: close_side,
                     }
                 });
                 
                 // CRITICAL FIX: Reset explicit flags so strategy knows it's free to quote again
                 // once position is confirmed closed (sync will handle actual qty)
                 self.has_active_buy = false;
                 self.has_active_sell = false;

                 // Retrying until position is 0 (handled by on_fill)
                 // self.last_trade_ts = None; // REMOVED to allow retry spam (with reduceOnly)
                 return Some(actions);
             } else {
                 // HOLDING: Do not quote new orders while holding (for SAFETY)
                 // But if we are holding and NOT closing (e.g. just gathering profit), we strictly wait.
                 // Once close signal triggers, we enter the block above.
                 return None;
             }
        }
        
        let bybit_bid = book.bids[0];
        let bybit_ask = book.asks[0];

        if bybit_bid.price == 0.0 || bybit_ask.price == 0.0 { 
            // println!("STRATEGY: Empty book, skip");
            return None; 
        }

        // Rate Limit: 1 Second
        let elapsed = self.last_update_ts.elapsed();
        if elapsed < Duration::from_millis(1000) {
            // println!("STRATEGY: Rate limited, {}ms elapsed", elapsed.as_millis());
            return None;
        }
        println!("STRATEGY: Generating orders! Elapsed: {}ms", elapsed.as_millis());

        // Logic: 0.3% away from BBO
        // Target Buy = Bid * 0.997
        // Target Sell = Ask * 1.003
        let target_buy_price = (bybit_bid.price * 0.997 * 100.0).round() / 100.0; // Round to 2 decimals (check tick size?)
        let target_sell_price = (bybit_ask.price * 1.003 * 100.0).round() / 100.0;
        
        // Size: Fixed 0.2 for test
        // let raw_qty: f64 = 12.0 / target_buy_price;
        // let buy_qty = raw_qty.max(1.0).round();
        let buy_qty = 0.2;
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
            self.active_buy_price = target_buy_price;
        } else {
             // Only amend if price changed
             if (self.active_buy_price - target_buy_price).abs() > 0.0001 {
                 actions.push(Action {
                    action_type: ActionType::AmendOrder {
                        price: target_buy_price,
                        qty: buy_qty,
                        side: "Buy",
                        link_id: self.active_buy_link_id.clone(),
                    }
                });
                self.active_buy_price = target_buy_price;
             }
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
            self.active_sell_price = target_sell_price;
        } else {
             // Only amend if price changed
             if (self.active_sell_price - target_sell_price).abs() > 0.0001 {
                 actions.push(Action {
                    action_type: ActionType::AmendOrder {
                        price: target_sell_price,
                        qty: buy_qty, // Sell same size
                        side: "Sell",
                        link_id: self.active_sell_link_id.clone(),
                    }
                });
                self.active_sell_price = target_sell_price;
             }
        }

        self.last_update_ts = Instant::now();
        
        if actions.is_empty() { None } else { Some(actions) }
    }

    pub fn reset_order(&mut self, side: &str) {
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros();
        if side == "Buy" {
            self.has_active_buy = false;
            self.active_buy_price = 0.0;
            self.active_buy_link_id = format!("b-{}", ts / 1000); // Millis
        } else if side == "Sell" {
            self.has_active_sell = false;
            self.active_sell_price = 0.0;
            self.active_sell_link_id = format!("s-{}", ts / 1000); // Millis
        }
    }
}
