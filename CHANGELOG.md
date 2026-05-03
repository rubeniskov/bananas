# CHANGELOG

All notable changes to BanaNAS are documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html), and entries are derived from [Conventional Commits](https://www.conventionalcommits.org/) by [semantic-release](https://github.com/semantic-release/semantic-release).

## 1.0.0 (2026-05-03)


### 🚀 Features

* **image:** bake bananas-config; narrow bananas-dashboard to bananapro ([28be922](https://github.com/rubeniskov/bananas/commit/28be9227d7c76f7d02450739afe9398050072dbe))
* **image:** slim bananas-image.bb to NAS-essential plugin set ([dca3819](https://github.com/rubeniskov/bananas/commit/dca3819b2090b8781d4b82de1b1905981d6473cf))
* **pixi:** add aarch64 cross-build + iterate-rpi dispatcher ([89fd590](https://github.com/rubeniskov/bananas/commit/89fd590773736e9d87a8ce53651e047ef5516311))
* **rpi:** add bananas-rpi unified machine + U-Boot dispatcher ([969a8b9](https://github.com/rubeniskov/bananas/commit/969a8b9ff50596a635373dcb5f622de1e42d74c1))
* **rpi:** widen plugin COMPATIBLE_MACHINE + per-arch FILESEXTRAPATHS ([82c4c6f](https://github.com/rubeniskov/bananas/commit/82c4c6f3992235220ebb51391266f2d9a6177ec3))


### 🐛 Bug Fixes

* **distro:** pin hostname to "bananas" across every MACHINE ([98cb13a](https://github.com/rubeniskov/bananas/commit/98cb13aa7d8c3df57ba191d4da265693b684668d))
* **rpi:** pin defconfig + grow FAT + license-flag + sunxi-bbappend mask ([2cffdd1](https://github.com/rubeniskov/bananas/commit/2cffdd1165244fe24322a38fb83cd3cf7a50289c))


### 🧹 Refactoring

* **kas:** drop kas.yml shim; require explicit board overlay ([5c92a8e](https://github.com/rubeniskov/bananas/commit/5c92a8e525494beae52e1f2b7105bb28b871019b))
* **kas:** split kas.yml into base + per-machine overlays ([8a36684](https://github.com/rubeniskov/bananas/commit/8a366842112bb23cc5acc1c17b1f7daf6f6fa79b))
* **machine:** rename bananapro MACHINE → bananas-bpi ([e619902](https://github.com/rubeniskov/bananas/commit/e619902cefd264e94b57ad255bc2056fb063b20e))


### 📚 Documentation

* **readme:** describe optional opkg plugin install ([dd0cfd5](https://github.com/rubeniskov/bananas/commit/dd0cfd570c8f9a8da2a5c9329df99d224170ef3f))
