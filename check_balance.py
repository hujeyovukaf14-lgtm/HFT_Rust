
def check_balance(filename):
    with open(filename, 'r') as f:
        lines = f.readlines()
    
    stack = []
    for i, line in enumerate(lines):
        line_num = i + 1
        for char in line:
            if char in '({[':
                stack.append((char, line_num))
            elif char in ')}]':
                if not stack:
                    print(f"Error: Unexpected closing {char} at line {line_num}")
                    return
                last, last_line = stack.pop()
                if (last == '(' and char != ')') or \
                   (last == '{' and char != '}') or \
                   (last == '[' and char != ']'):
                    print(f"Error: Mismatched {char} at line {line_num} (opened {last} at {last_line})")
                    return
    
    if stack:
        print(f"Error: Unclosed {stack[-1][0]} at line {stack[-1][1]}")
    else:
        print("Balance OK")

check_balance('d:\\HFT_Rust\\src\\main.rs')
