# V2 Package Registry

This directory is a V2 package registry index. Each `index/<name>.toml` maps a
package name to its source; each `packages/<name>/` is a V2 package.

It hosts the **reference (non-core) standard library** — the modules that are not
built into the `v2` binary and are instead delivered as installable packages.

## Use it

```bash
export V2_REGISTRY=/path/to/this/registry   # or a GitHub URL of this repo
v2 add std.http
v2 search http
```

## Packages (62)

- `std.2d`
- `std.accessibility`
- `std.ai`
- `std.archive`
- `std.audio`
- `std.barcode`
- `std.blockchain`
- `std.bluetooth`
- `std.camera`
- `std.cli`
- `std.clipboard`
- `std.compress`
- `std.db`
- `std.diag`
- `std.dns`
- `std.embed`
- `std.excel`
- `std.ffi`
- `std.game`
- `std.geo`
- `std.gfx3d`
- `std.gpu`
- `std.graphql`
- `std.grpc`
- `std.hal`
- `std.hotkey`
- `std.http`
- `std.i18n`
- `std.image`
- `std.iot`
- `std.ipc`
- `std.jwt`
- `std.mail`
- `std.map`
- `std.markdown`
- `std.ml.audio`
- `std.ml.vision`
- `std.mqtt`
- `std.multipart`
- `std.net`
- `std.notify`
- `std.oauth2`
- `std.office`
- `std.pdf`
- `std.phone`
- `std.proc`
- `std.qr`
- `std.scrape`
- `std.serial`
- `std.speech`
- `std.ssh`
- `std.task`
- `std.template`
- `std.term`
- `std.tray`
- `std.ui`
- `std.usb`
- `std.video`
- `std.watch`
- `std.webrtc`
- `std.xml`
- `std.yaml`
