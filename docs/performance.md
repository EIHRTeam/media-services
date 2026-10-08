# Performance measurements

Measured 2026-10-08 on Apple M4 Pro (12 available CPUs), macOS arm64, Node.js
24.21.0. All 64 scenarios completed four successful images with no errors.
These are short local measurements, not portable throughput guarantees.

## Container allocations

The allocator example measures only rewriting: inputs are already resident.
The baseline is commit `7227b06`, extracted to a temporary
standalone program. Both programs use the same allocator instrumentation;
compare allocation counts rather than timings across their different build setups.

| 128 MiB image | Previous peak / cumulative allocation | New peak / cumulative allocation |
| ------------- | ------------------------------------- | -------------------------------- |
| WebP          | 402,653,480 / 402,653,480 bytes       | 134,217,898 / 134,217,898 bytes  |
| AVIF          | 134,219,681 / 268,441,143 bytes       | 134,218,990 / 134,220,363 bytes  |

WebP removes two simultaneously resident image intermediates: about 67% less
rewrite allocation peak. AVIF removes a sequential full-image assembly: about
50% less cumulative allocation, with an essentially unchanged image-sized peak.
Both now assemble one final image buffer. The committed example asserts peak
and cumulative allocation ceilings for its fixture, so it catches both patterns.
The 32, 128 and 256 MiB cases passed locally; CI runs 32 MiB.

## Node signing pipeline

Each scenario runs in a fresh process with a warm-up before timing. Startup
cost is recorded separately. The input budget is fixed at 512 MiB, so 256 MiB
jobs admit at most two running inputs even with four Workers. The fixtures keep
real encoded image payloads and grow valid WebP ancillary/AVIF free boxes; they
measure copying and hashing, not decoding or encoding. Signing uses a local mock
service. Main-thread external-memory samples exclude most Worker heaps; process
RSS high-water marks and reported WASM memory cover those costs more usefully.

| Input        | Workers | Input MiB/s | Maximum event-loop delay (ms) | Peak process RSS (MiB) |
| ------------ | ------: | ----------: | ----------------------------: | ---------------------: |
| 32 MiB WEBP  |       0 |         179 |                         569.4 |                    711 |
| 32 MiB WEBP  |       1 |         181 |                           2.0 |                    522 |
| 32 MiB WEBP  |       2 |         213 |                           1.8 |                    928 |
| 32 MiB WEBP  |       4 |         228 |                           1.9 |                   1490 |
| 32 MiB AVIF  |       0 |        1947 |                          29.7 |                    704 |
| 32 MiB AVIF  |       1 |        2267 |                           2.2 |                    513 |
| 32 MiB AVIF  |       2 |        3045 |                           1.5 |                    845 |
| 32 MiB AVIF  |       4 |        3600 |                           1.9 |                   1315 |
| 256 MiB WEBP |       0 |         166 |                        4722.8 |                   3056 |
| 256 MiB WEBP |       1 |         172 |                           6.0 |                   2402 |
| 256 MiB WEBP |       2 |         222 |                           4.6 |                   3111 |
| 256 MiB WEBP |       4 |         206 |                           6.2 |                   3219 |
| 256 MiB AVIF |       0 |        1122 |                         546.3 |                   2957 |
| 256 MiB AVIF |       1 |        2763 |                           2.1 |                   2381 |
| 256 MiB AVIF |       2 |        1104 |                           6.6 |                   3456 |
| 256 MiB AVIF |       4 |        1593 |                           2.4 |                   3338 |

Workers consistently reduce main-thread blocking. More Workers do not guarantee
more throughput: memory bandwidth, independent WASM heaps and signing format
costs can dominate. For large files, begin with one Worker and increase after
measurement; for smaller batches, compare two Workers before enabling four.
A 256 MiB image can still require several GiB of process memory in the C2PA
pipeline. Scheduling byte budgets are not an RSS cap.

The regression suite separately exercises a 100-item lazy batch with a slow
consumer and asserts bounded pulling/loading and concurrent input reservations.
It also covers early exit, cancellation, queue saturation, clone failures,
Worker replacement and deadline-based shutdown.

## Reproduce

```sh
pnpm build
pnpm gen:identity
pnpm bench
cargo run -p xmp-image --example memory --release --locked -- 128
```

Full records include throughput, startup, event-loop p95/max, RSS, sampled main
thread external/ArrayBuffer memory and reported WASM bytes. They are written to
`target/tmp/benchmark.json`; Cargo may clear that directory when rebuilding.
Use `MPS_BENCH_SIZES=32 MPS_BENCH_ITEMS=4 pnpm bench` for a short comparison.

Independent signature and asset-hash checks use c2patool 0.28.1. Both native
artifacts and the Node/Worker/CLI outputs passed locally. Public certificate
trust is excluded for the generated test CA.
