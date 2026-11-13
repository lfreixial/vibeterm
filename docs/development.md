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

GitHub Actions runs checks, builds tagged releases, and publishes the website.
No AUR publication is automated.

## Releasing

1. Update the application version in `Cargo.toml` and `Cargo.lock`, add a
   `CHANGELOG.md` section, and update README/website download links. Keep the Arch
   recipe pinned to the previous tag until the new source archive can be hashed.
2. Merge to `main`, then tag and push: `git tag v0.2.0 && git push origin v0.2.0`.
3. `.github/workflows/release.yml` checks the tag matches `Cargo.toml`, runs the
   tests, builds on Ubuntu 24.04 (so older glibc versions can run it) and publishes a
   full release marked Latest for stable tags, or a pre-release for tags with a
   suffix (such as `v0.2.0-rc.1`). Both include the binary tarball, its SHA-256, and
   the changelog section as notes.
4. After publishing, update `pkgver` in the PKGBUILD and set `sha256sums` to the
   digest of GitHub's source tarball for the tag: `curl -sfL https://github.com/lfreixial/vibeterm/archive/refs/tags/v0.2.0.tar.gz | sha256sum`
   (`-f` matters: without it an error page would be hashed). Confirm with `makepkg -f`.

To test the package before tagging, build a tarball laid out like GitHub's:
`git archive --prefix=vibeterm-0.2.0/ -o packaging/vibeterm-0.2.0.tar.gz HEAD`, then
`cd packaging && makepkg -f --skipchecksums`.
