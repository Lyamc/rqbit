# Build/run environment for the GPUI desktop client on NixOS (or any Nix
# system) without changing the system configuration:
#
#   nix-shell crates/rqbit-gpui/shell.nix --run \
#     'cargo build --release -p rqbit --no-default-features --features rust-tls,webui,prometheus,gpui'
#   nix-shell crates/rqbit-gpui/shell.nix --run 'target/release/rqbit gui --url http://127.0.0.1:3030'
#
# Rust itself comes from your usual toolchain (rustup), not from this shell.
{ pkgs ? import <nixpkgs> { } }:
let
  runtimeLibs = with pkgs; [
    libxkbcommon
    wayland
    vulkan-loader
    fontconfig
    freetype
    xorg.libX11
    xorg.libxcb
    xorg.libXcursor
    xorg.libXrandr
    xorg.libXi
  ];
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [ pkg-config ];
  buildInputs = runtimeLibs;
  # GPUI/blade dlopen() Vulkan, Wayland and X11 libraries at runtime.
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibs;
}
