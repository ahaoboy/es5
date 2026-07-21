# es5

A [MuJS](https://mujs.com/)-compatible ES5 JavaScript interpreter written in Rust.

## Quick start

```bash
cargo binstall https://github.com/ahaoboy/es5

cargo build --release

# Evaluate an expression
es5 -e "print(1 + 2)"

# Execute a script
es5 script.js

# Evaluate via stdin
echo "print(1 + 2)" | es5

# Interactive REPL
es5 -i
```

## Related

- [MuJS](https://mujs.com/) — the original C interpreter
- Based on MuJS 1.3.8, adjusted toward stricter ES5 conformance

## License

MIT
