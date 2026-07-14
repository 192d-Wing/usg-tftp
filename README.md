# USG TFTP

USG TFTP is a reusable, standalone TFTP server, client, and Rust library. It supports RFC 1350 transfers and the option extensions documented under [`docs/`](docs/).

## Build and test

```sh
cargo build
cargo test
```

## Run

```sh
cargo run --bin usg-tftp-server -- --help
cargo run --bin usg-tftp-client -- --help
```

Configuration examples are available under [`examples/`](examples/).

## License

Licensed under either the MIT License or Apache License 2.0, at your option.
