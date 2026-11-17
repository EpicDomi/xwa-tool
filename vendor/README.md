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

## iced_winit 0.14.1

From <https://crates.io/crates/iced_winit/0.14.1> (iced, MIT licence),
with three changes:

- `src/lib.rs`, resizing a window: winit applies a requested size at once on
  Wayland and returns it, sending no `Resized` event, as its documentation
  allows; iced dropped the returned size, so the drawing surface kept its
  old size after the app resized the window. KDE then stretched the old
  picture over the new window, cropped at the right and bottom, and clicks
  landed off target by more the further they were from the top left. The
  returned size is now handled as the event would be.
- `src/conversion.rs`, opening a window centred: iced opens the window at its
  size times the app's scale factor, but centred it as if it were unscaled,
  so a window opened below its natural size sat too high on the screen,
  and one opened above it too low.
- `Cargo.toml`: the crate is published with two deprecation warnings, which
  a crates.io dependency never shows; as a path dependency they would show
  on every build, so they are allowed.
