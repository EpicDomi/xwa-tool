# Vendored crates

Crates from crates.io with small changes xwa-tool needs, used in place of
the published ones through `[patch.crates-io]` in the workspace's
`Cargo.toml`. Each keeps its own licence. When updating one, take the new
release's published files and apply the same changes again; every change
is marked in the source with a comment starting `Patched for xwa-tool`.

## iced_wgpu 0.14.0

From <https://crates.io/crates/iced_wgpu/0.14.0> (iced, MIT licence), with
two changes in `src/window/compositor.rs`:

- The wgpu instance takes its backend options from the environment
  (`wgpu::BackendOptions::from_env_or_default()`), as wgpu's own examples
  do. Unless a `WGPU_*` variable is set this changes nothing. xwa-tool sets
  `WGPU_DX12_PRESENTATION_SYSTEM=Visual` on Windows, so DirectX 12 presents
  through DirectComposition, the way Windows composes transparent windows.
- The surface prefers premultiplied alpha to post-multiplied. iced's
  pipelines blend premultiplied colours, and DirectComposition expects
  premultiplied alpha. Metal offers post-multiplied only and OpenGL opaque
  only, so neither changes; on Vulkan, premultiplied is what iced chose
  already wherever post-multiplied was not offered as well.
