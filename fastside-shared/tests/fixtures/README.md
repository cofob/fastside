# Anubis compatibility vectors

`anubis-vectors.json` was generated from the local Anubis source at commit
`4578023de7b631537e3a43d89b1998e802beb7e0` (1.28.0-pre1).

A separate Rust program ran the `compute_hash`, `make_hashx`, and `validate`
functions from `wasm/pow/{sha256,argon2id,hashx}/src/lib.rs`. The only adaptation
was to pass the challenge bytes as a function argument instead of reading the
WASM data buffer. It used sha2 0.10.9, argon2 0.5.3, and hashx 0.8.0.

Each result is the first nonce from zero that meets a two-bit difficulty.
The two inputs cover both nonce byte orders. These expected values are
independent of the fastside solver.
