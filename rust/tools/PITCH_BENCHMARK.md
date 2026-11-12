# Pitch analysis validation

Version: 0.4.7. Validation performed on 2026-10-05, Windows x64, optimized Rust builds. These measurements describe host-side algorithm work; they are not Android device or battery measurements.

The recorded old/new timing comparison used `rustc.exe` resolved from PATH, with the default `stable-x86_64-pc-windows-gnu` toolchain. Compiler details: rustc 1.95.0, commit `59807616e1fa2540724bfbac14d7976d7e4a3860` dated 2026-04-14, host `x86_64-pc-windows-gnu`, LLVM 22.1.2. Both the old and optimized timing binaries were compiled with this same compiler and `--edition=2021 -O`. The old source was preserved temporarily for the comparison and deleted after validation.

## Reproducible timing workload

From the project root, reproduce the optimized workload with the GNU compiler used for the comparison:

```powershell
rustc +stable-x86_64-pc-windows-gnu --edition=2021 -O rust/tools/pitch_benchmark.rs -o "$env:TEMP\pitch_benchmark.exe"
& "$env:TEMP\pitch_benchmark.exe"
```

The MSVC toolchain used for the complete project release tests can also compile this workload:

```powershell
rustc +stable-x86_64-pc-windows-msvc --edition=2021 -O rust/tools/pitch_benchmark.rs -o "$env:TEMP\pitch_benchmark.exe"
& "$env:TEMP\pitch_benchmark.exe"
```

The tool warms one reusable analyzer, then processes 2400 frames: six open-string frequencies, 4096 signed 16-bit PCM samples per frame, 48 kHz, fundamental plus second and third harmonics. It reports elapsed wall time, a frequency checksum, and heap allocation count after warmup. Use the same machine and compiler options for comparisons.

MSVC and GNU timing results need separate baselines because their code generation and runtime differ. A follow-up MSVC run while three Android ABI builds were compiling measured 1314.820 ms with checksum 444780.875 and zero warm-frame allocations. That run includes host CPU contention and cannot establish a performance regression or be compared directly against the GNU baseline below.

| Workload | Before optimization | After optimization |
| --- | ---: | ---: |
| Interleaved timing run 1 | 2842.172 ms | 466.038 ms |
| Interleaved timing run 2 | 2578.530 ms | 434.951 ms |
| Frequency checksum | 444780.875 | 444780.875 |
| Measured warm-frame allocations | Not measured | 0 |

The second interleaved run uses approximately 83.1% less analysis time, or 5.93 times the throughput. Timing is sensitive to other work on the host.

## Accuracy and domain checks

All 27 pitch/tuning domain tests passed. The complete project release test run passed 34 tests. The added tests cover rejection below 60 Hz, cache reuse across sample-rate and frame-size changes, detuned strings, rejection of silent or low-confidence reading metadata, and fail-closed handling of unfinished periods in short high-rate frames.

A separate 1008-frame comparison covered all six strings at -100, -30, -15, 0, +15, +30, and +100 cents, both 44.1 and 48 kHz, 2048/4096/8192 samples, two phases, and pure/harmonic-rich signals. Old and new detection presence agreed on every frame. Maximum differences were:

| Quantity | Maximum difference |
| --- | ---: |
| Old/new detected pitch | 0.000413 cents |
| New detected pitch versus generated pitch | 0.869670 cents |
| Harmonic support | 0.000000119 |
| Subharmonic support | 0.000005107 |
| Confidence | 0.000000060 |

The pre-change analyzer incorrectly returned 60 to 60.606 Hz for 59, 59.5, and 59.9 Hz input. The updated boundary test rejects those tones at 16, 44.1, 48, and 96 kHz, while 60.2 Hz remains accurate within 0.03 Hz.

An independent review also found that 2048-sample E2 frames at 88.2/96 kHz could be misreported near 88.55/96.39 Hz because the search stopped before a complete period minimum. The updated analyzer rejects those unfinished periods; 4096-sample versions remain accurate within 1 cent. This extra boundary guard was added after the recorded timing comparison; it does not affect the normal 48 kHz benchmark workload.

Independent post-fix review passed 78 boundary cases and 360 production-condition cases. For E2 at 88.2/96 kHz, all tested phases rejected 2048/2049-sample unfinished periods and accepted 4095/4096/4097/8192 samples with at most 0.349571 cents error. Across seven supported sample rates, 59/59.5/59.9 Hz were rejected, while 60.2/60.5/61 Hz were accepted with at most 0.006516 Hz error. The 360 production cases had no missed detections and at most 0.807167 cents error.

Independent numerical checks also verified bit-identical cached/fresh analyzer outputs across 336 cases and a maximum Goertzel-versus-f64-direct-DFT relative power error of 0.0000000507 across 128 cases.

## Mutation evidence

The original source fails both new bug reproductions: a 59.9 Hz tone is accepted, and a reading with zero confidence and zero RMS sets a target. Changing only displayed string descriptions leaves all 27 domain tests passing. Removing the reading-validation gate causes the silent/unclear-reading domain test to fail. Deleting the unfinished-period boundary gate makes the high-rate short-frame test fail. All mutations used temporary copies; production source retains both validation gates.

Recorded audio and explicitly authorized physical-device testing remain necessary to assess microphone behavior, room noise, Android scheduling and total power consumption.
