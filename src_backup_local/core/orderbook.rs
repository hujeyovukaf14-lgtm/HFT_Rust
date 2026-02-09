use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct Level {
    pub price: f64,
    pub qty: f64,
}

/// L2 OrderBook with fixed depth (20 levels).
/// Aligned to 64 bytes to fit in cache lines and avoid false sharing.
/// Memory Layout: 20 * 16 bytes (bids) + 20 * 16 bytes (asks) = 640 bytes. 
/// Fits easily in L1.
#[repr(C, align(64))]
pub struct L2OrderBook {
    pub bids: [Level; 20],
    pub asks: [Level; 20],
}

impl Default for L2OrderBook {
    fn default() -> Self {
        Self {
            bids: [Level::default(); 20],
            asks: [Level::default(); 20],
        }
    }
}

impl L2OrderBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates the orderbook.
    /// This is a simplified "Insert/Update" O(N) implementation for fixed array.
    /// For HFT with 20 levels, linear scan is often faster than B-Tree pointers due to prefetching.
    /// 
    /// Note: This implementation assumes updates come in random order. 
    /// If qty == 0.0, remove the level.
    pub fn update(&mut self, side: Side, price: f64, qty: f64) {
        let levels = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };

        // 1. Try to find existing level to update or remove
        for i in 0..20 {
            if (levels[i].price - price).abs() < f64::EPSILON {
                if qty == 0.0 {
                    // Remove: shift remaining elements up
                    // memmove style shift
                    for j in i..19 {
                        levels[j] = levels[j+1];
                    }
                    levels[19] = Level::default(); // clear last
                } else {
                    // Update
                    levels[i].qty = qty;
                }
                return;
            }
        }

        // 2. Insert new level (if not found and qty > 0)
        // This requires maintaining sort order.
        // Bids: Descending (Highest buy first)
        // Asks: Ascending (Lowest sell first)
        // If book is full and new price is worse than worst level, ignore.
        
        if qty == 0.0 { return; } // Removing non-existent level, ignore.

        match side {
            Side::Buy => {
                // Find insertion point for DESCENDING order
                for i in 0..20 {
                    // Empty slot found or better price found
                    if levels[i].price == 0.0 || price > levels[i].price {
                         // Shift right
                         for j in (i+1..20).rev() {
                             levels[j] = levels[j-1];
                         }
                         levels[i] = Level { price, qty };
                         return;
                    }
                }
            },
            Side::Sell => {
                // Find insertion point for ASCENDING order
                for i in 0..20 {
                    // Empty slot (price check 0.0 works effectively if initialization is 0)
                    // Or found a higher price (we are lower, so we go before it)
                    if levels[i].price == 0.0 || price < levels[i].price {
                        // Shift right
                         for j in (i+1..20).rev() {
                             levels[j] = levels[j-1];
                         }
                         levels[i] = Level { price, qty };
                         return;
                    }
                }
            }
        }
    }
}

// Display for debugging
impl fmt::Display for L2OrderBook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "ASKS:")?;
        for i in (0..5).rev() {
             if self.asks[i].price > 0.0 {
                writeln!(f, "{:.2} | {:.4}", self.asks[i].price, self.asks[i].qty)?;
             }
        }
        writeln!(f, "-----")?;
        for i in 0..5 {
             if self.bids[i].price > 0.0 {
                writeln!(f, "{:.2} | {:.4}", self.bids[i].price, self.bids[i].qty)?;
             }
        }
        writeln!(f, "BIDS")
    }
}
