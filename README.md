# es5

A [MuJS](https://mujs.com/)-compatible ES5 JavaScript engine written in Rust.

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

## Benchmark

es5 is regularly benchmarked against other lightweight JavaScript engines, including [Boa](https://github.com/boa-dev/boa), [MuJS](https://mujs.com/), and [Lumen](https://github.com/lucid-softworks/lumen).

**Live results:** [js-engine-benchmark](https://ahaoboy.github.io/js-engine-benchmark/?selectEngines=lumen%2Cmujs%2Cboa%2Ces5)


## React

[`es5-react`](https://github.com/ahaoboy/es5-react) is a library that renders React components to the terminal, powered by es5 as its JavaScript runtime.



## Related

- [MuJS](https://mujs.com/) — the original C interpreter
- Based on MuJS 1.3.8, adjusted toward stricter ES5 conformance

## License

MIT
