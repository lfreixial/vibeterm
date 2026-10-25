# Development

Normal checks:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release --locked
```

The environment initially had no Rust compiler. It was installed only in `/tmp`,
without editing shell startup files or installing system packages. Reuse it before temporary files expire:

```sh
export RUSTUP_HOME=/tmp/vibeterm-rustup
export CARGO_HOME=/tmp/vibeterm-cargo
export PATH=/tmp/vibeterm-rustup/toolchains/1.90.0-x86_64-unknown-linux-gnu/bin:$PATH
export RUSTC=/tmp/vibeterm-rustup/toolchains/1.90.0-x86_64-unknown-linux-gnu/bin/rustc
export RUSTDOC=/tmp/vibeterm-rustup/toolchains/1.90.0-x86_64-unknown-linux-gnu/bin/rustdoc
cargo test --workspace --locked
```

The host sandbox blocks wake-socket writes/signals and initially hung PTY tests.
Those were terminated and passed with real process permissions. Initialization now
probes its wake socket before spawning, returning a clear error if blocked.
Compilation/non-PTY tests work without desktop access; GUI tests ran outside the sandbox.

The X11 smoke script finds its PID-owned window and sends keyboard events directly
to it. The X input-method filter discards key events for an unfocused window, so the
script restores focus to its window before each key; focus taken by another app
mid-run previously made random checks fail (likely the earlier "intermittent resize
timeout"). Build examples and pass `--workspace` for tabs/splits/search/palette/live
config checks; these also exercise the clipboard without pointer automation.
Optional pointer tests check window ancestry before clicking and restore
position afterward. KWin prevented pointer targeting here; manual acceptance remains.

Fixtures: `visual-fixture.py` emits ANSI/Unicode; `throughput.py` emits a fixed load;
`examples/replay.rs` measures parser throughput alone; `measure.py` samples idle
CPU/RSS and release diagnostics. Do not equate those figures with physical latency.

The GitHub workflow is added but has not run remotely. Packaging is local only;
no package installation or AUR publication was performed.

## Releasing

1. Set `version` in `Cargo.toml` and `pkgver` in `packaging/PKGBUILD`, run
   `cargo build --locked` so `Cargo.lock` matches, and add a `CHANGELOG.md` section.
2. Merge to `main`, then tag and push: `git tag v0.1.0 && git push origin v0.1.0`.
3. `.github/workflows/release.yml` checks the tag matches `Cargo.toml`, runs the
   tests, builds on Ubuntu 24.04 (so older glibc versions can run it) and publishes a
   pre-release with the binary tarball, its SHA-256 and the changelog section as notes.
4. Replace `sha256sums=('SKIP')` in the PKGBUILD with the digest of GitHub's source
   tarball for the tag (`makepkg -g` prints it).

To test the package before tagging, build a tarball laid out like GitHub's:
`git archive --prefix=vibeterm-0.1.0/ -o packaging/vibeterm-0.1.0.tar.gz HEAD`, then
`cd packaging && makepkg -f --skipchecksums`.
