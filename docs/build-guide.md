# Build Guide
This is for those who want to build `rathole` themselves, possibly because the need of latest features or the minimal binary size.

## Build
To use default build settings, run:
```
cargo build --release
```

## Customize the build
`rathole` comes with lots of *crate features* that determine whether a certain feature will be compiled or not. Supported features can be checked out in `[features]` of [Cargo.toml](../Cargo.toml).

For example, to build `rathole` with the `client` and `noise` feature:
```
cargo build --release --no-default-features --features client,noise
```

## Minimalize the binary

1. Build with the `minimal` profile

The `release` build profile optimize for the program running time, not the binary size. 

However, the `minimal` profile enables lots of optimization for the binary size to produce a much smaller binary.

For example, to build `rathole` with `client` feature with the `minimal` profile:
```
cargo build --profile minimal --no-default-features --features client
```

2. `strip` and `upx`

The binary that step 1 produces can be even smaller, by using `strip` and `upx` to remove the symbols and compress the binary.

Like:
```
strip rathole
upx --best --lzma rathole
```

At the time of writting the build guide, the produced binary for `x86_64-unknown-linux-glibc` has the size of **574 KiB**, while `frpc` has the size of **~10 MiB**, which is much larger.
