# Web version and deployment — design

Status: approved in conversation, 2026-10-14.

## Goal

Run ER Nuance in the browser as a public, local-only tool at `https://er.smartnuance.com`: open,
drop or paste a diagram (or two, to diff them); everything stays in the browser. The server only
serves static files. Deployed to the `contentnuance` cluster with manifests in this repository.

## Non-goals

- Loading diagrams from URLs or share links (option B/C), server-side git or private repositories.
- Git diffs (HEAD, commits, index), file watching and live reload in the browser.
- CI that builds or deploys the image (deploys stay manual, like the other apps on the cluster).

## App: one crate, two targets

The crate builds for native (`ernuance` binary, unchanged behaviour) and `wasm32-unknown-unknown`.

Native-only, behind `cfg(not(target_arch = "wasm32"))` (code and dependencies):

- `cli.rs` and `clap`; `vcs.rs`'s git access and `gix`; `native_menu.rs` and `muda` (macOS, as now)
- file paths, mtime polling, the `.layout.json` sidecar, `rfd`'s blocking dialogs

New, portable:

- An in-memory source `{ name, text }`. `Source::Text` (view) and a `Side::Text` (diff side); on
  the web these are the only sources, on native they are used for pasted text.
- `Side` and `read` move to a portable `source.rs`-style split: `Side::Text` everywhere,
  `Side::File` / `Side::Git` native-only. Diff code (`diff.rs`) is unchanged.
- Layout persistence: native keeps the sidecar; web stores `SavedLayout` JSON in `localStorage`
  under `ernuance.layout.<file name>`. Same `SavedLayout` struct.
- Opening on the web: `rfd::AsyncFileDialog` run with `wasm_bindgen_futures::spawn_local`; the result
  comes back through a shared slot polled each frame. Drag & drop uses egui's dropped-file bytes.
- "Paste diagram…": a window with a multi-line text field and *Open*; available on both targets.
- "Compare with file…" on the web: picks a file and compares it (old) with the open one (new).
- Menus and palette hide what does not apply (reload, compare with HEAD / commit, exit diff to a
  file path) on the web. The welcome screen says files never leave the browser.

Entry point: `main.rs` keeps the native `main`; on wasm, `main` starts `eframe::WebRunner` on a
`<canvas id="the_canvas_id">` and hides the loading text once started.

## Build

- `index.html` and `Trunk.toml` at the repository root; `trunk build --release` writes to
  `target/web/`. Release profile for the web: size-optimised (`opt-level = "s"` via trunk
  data-cargo-profile or a `web-release` profile), `wasm-opt` from trunk.
- `Dockerfile` (multi-stage): `rust` + `wasm32-unknown-unknown` + trunk → `nginxinc/nginx-unprivileged`
  (port 8080, non-root). nginx config: `application/wasm` type, gzip for wasm/js/html,
  `Cache-Control: no-cache` for `index.html`, one year immutable for hashed assets.
- Image `ghcr.io/smartnuance/er-nuance:<version>` and `:latest`, `--platform linux/amd64`, public
  package (no pull secret).
- `Makefile.toml` (cargo-make, like smartping): `web-serve` (trunk serve), `web-image`, `web-push`,
  `web-deploy` (sets the image tag in `k8s/deployment.yaml`, `kubectl --context contentnuance
  apply -k k8s/`).

## Kubernetes (`k8s/`)

- `namespace.yaml`: `er-nuance`.
- `deployment.yaml`: 1 replica, `app: er-nuance`, image above, `imagePullPolicy: Always`, port 8080,
  requests 10m / 32Mi, limits 100m / 64Mi, readiness and liveness HTTP probes on `/`,
  `runAsNonRoot`, read-only root filesystem with an `emptyDir` for nginx's temp paths.
- `service.yaml`: ClusterIP 80 → 8080.
- `ingress.yaml`: host `er.smartnuance.com`, `ingressClassName: nginx`,
  `cert-manager.io/cluster-issuer: letsencrypt`, `nginx.ingress.kubernetes.io/ssl-redirect: "true"`,
  TLS secret `er-nuance-tls`.
- `kustomization.yaml`: `namespace: er-nuance`, the four resources.

Manual steps (documented in the README, not automated): DNS A record for `er.smartnuance.com` to
the ingress load balancer, making the ghcr package public, running `cargo make web-deploy`.

## Testing

- Native: all existing tests pass; unit test for reading a `Side::Text`; clippy clean.
- Web: `cargo check` and `cargo clippy --target wasm32-unknown-unknown` clean.
- Image: built and run locally; curl checks `Content-Type: application/wasm`, caching headers and
  that `index.html` loads; the page renders once in a browser.
- Manifests: `kubectl kustomize k8s/` renders without errors.
- Not done by Claude without asking: pushing the image, applying to the cluster.
