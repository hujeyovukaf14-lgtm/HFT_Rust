import sys

path = r"d:\HFT_Rust\src\main.rs"
with open(path, "r", encoding="utf-8") as f:
    content = f.read()

replacements = [
    ('println!("HOT: Binance Upgraded!");', 'if !minimal_logging { println!("HOT: Binance Upgraded!"); }'),
    ('println!("HOT: Private Active entered, end={}", end);', 'if !minimal_logging { println!("HOT: Private Active entered, end={}", end); }'),
    ('println!("HOT: Sending Trade Handshake...");', 'if !minimal_logging { println!("HOT: Sending Trade Handshake..."); }'),
    ('println!("HOT: Authenticating Trade WS...");', 'if !minimal_logging { println!("HOT: Authenticating Trade WS..."); }'),
    ('println!("HOT: Trade Switch Proto!");', 'if !minimal_logging { println!("HOT: Trade Switch Proto!"); }'),
    ('println!("HOT: Trade RAW: {}", payload_str);', 'if !minimal_logging { println!("HOT: Trade RAW: {}", payload_str); }'),
    ('println!("HOT: Trade WS AUTHENTICATED!");', 'if !minimal_logging { println!("HOT: Trade WS AUTHENTICATED!"); }'),
    ('println!("HOT: Trade -> Position already closed (110017). Syncing to 0.");', 'if !minimal_logging { println!("HOT: Trade -> Position already closed (110017). Syncing to 0."); }')
]

count = 0
for old, new in replacements:
    if old in content:
        content = content.replace(old, new)
        count += 1
    else:
        print(f"Not found: {old}")

with open(path, "w", encoding="utf-8") as f:
    f.write(content)

print(f"Replaced {count} items.")
