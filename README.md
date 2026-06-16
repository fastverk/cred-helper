# fastverk/cred-helper

The fastverk **universal Bazel credential helper** and its reusable
`credresolve` core. Resolves the auth header for a Bazel fetch URI from a
host→connection registry, through pluggable secret backends — **keychain**
(local/mac), **env vars** (CI), and **file** — degrading to anonymous on any
miss so a fetch never fails the build.

## Surfaces

| What | Where |
|---|---|
| **`credresolve`** (library) | the contract: `connection.proto` schema, the read/resolve path, and the `SecretStore` backends. `prost`-only, dependency-light. The single source of truth (fvkit layers `connect`/OAuth on top). |
| **`cred-helper`** (binary) | a ~40-line wrapper implementing the Bazel credential-helper protocol over `credresolve::resolve`. |
| **Prebuilt release artifacts** | `cred-helper-{linux-amd64,darwin-arm64}` + `cred-helper-linux-amd64-layer.tar` (`/usr/local/bin/cred-helper`, 0755), published per commit. |

## Consume the prebuilt helper

Public releases — fetch with **no auth**. Pin the **immutable** `credhelper-<sha>`
tag (not the rolling `credhelper-latest`):

```starlark
http_file(
    name = "fastverk_cred_helper_layer",
    urls = ["https://github.com/fastverk/cred-helper/releases/download/credhelper-<sha>/cred-helper-linux-amd64-layer.tar"],
    sha256 = "<from cred-helper-sha256.txt>",
    downloaded_file_path = "cred-helper-linux-amd64-layer.tar",
)
```

Add `@fastverk_cred_helper_layer//file` to your image `tars`, keep an unscoped
`--credential_helper=/usr/local/bin/cred-helper`.

## Runtime auth (credential-free artifacts)

Tokens are injected as **environment variables** per consuming CI job; the
helper resolves them at runtime. First non-empty wins:

- GitHub hosts → `GITHUB_TOKEN` / `GH_TOKEN` → `Authorization: Bearer`
- GitLab (gitlab.com) → `GITLAB_TOKEN` → `Authorization: Bearer`
- BuildBuddy → `BUILDBUDDY_API_KEY` → `x-buildbuddy-api-key`
- canonical form for any built-in connection: `FASTVERK_TOKEN_<ID>`
- **any other host** (e.g. a self-hosted GitLab) → `FASTVERK_TOKEN_<HOST>` (host
  uppercased, non-alphanumerics → `_`, e.g. `FASTVERK_TOKEN_GIT_EXAMPLE_COM`) →
  `Authorization: Bearer`. Nothing host-specific is baked into the tool.

On mac/local the helper reads the OS **Keychain** (via the fastverk app's
connection registry), which takes precedence over env.

## Build

```sh
bazel test //...                                                   # host
bazel build //cred-helper:cred_helper_layer --platforms=//tools/oci:linux_amd64   # linux/amd64 layer
```
