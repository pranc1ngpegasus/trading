# tws-api

Rust types for the Interactive Brokers TWS wire protocol, generated with `buffa` at build time.
The build script reads every `.proto` under `$TWS_API_ROOT/source/proto` and writes the Rust into `OUT_DIR`, so no generated code enters git.
The dev shell provides both the proto tree and `protoc`.
Run `nix develop -c cargo test -p tws-api` to build and test the crate.
The upstream protos are GPLv3 and are never copied into this MIT repository.
