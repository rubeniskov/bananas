# CHANGELOG

All notable changes to BanaNAS are documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html), and entries are derived from [Conventional Commits](https://www.conventionalcommits.org/) by [semantic-release](https://github.com/semantic-release/semantic-release).

## [1.0.1](https://github.com/rubeniskov/bananas/compare/v1.0.0...v1.0.1) (2026-04-30)


### 🐛 Bug Fixes

* **release:** drop 4-byte unicode from SD-card asset label ([aa4018b](https://github.com/rubeniskov/bananas/commit/aa4018b2f758068dc2b04b0c32c9b126455bd0c2))

## 1.0.0 (2026-04-30)


### 🚀 Features

* **auth:** force root to rotate password on first sign-in ([447bc45](https://github.com/rubeniskov/bananas/commit/447bc45a9b682c7a192d92de631e5855c613ee14))
* **ci:** wire prek (pre-commit hooks) for local + CI gating ([137b374](https://github.com/rubeniskov/bananas/commit/137b37437275b25935fb09f69ea5d141b1f6a5e7))
* **cloud:** account + sync-entry config storage and management UI ([69954a1](https://github.com/rubeniskov/bananas/commit/69954a1f50ca7ec6685cdb29d187e6a3cdd77299))
* **cloud:** async runs + cron-driven scheduled syncs ([818da11](https://github.com/rubeniskov/bananas/commit/818da112056d7070deeeae0d212ff1964e64fdf1))
* **cloud:** cancel button + reposition the live progress to the label cell ([b9a7151](https://github.com/rubeniskov/bananas/commit/b9a715173d39193f7efe1747cbd663fba33992bb))
* **cloud:** edit existing accounts (rotate token / change provider) ([90c75ac](https://github.com/rubeniskov/bananas/commit/90c75ac5733daf7b2dd2fe149605d67f8b2a9fd0))
* **cloud:** live circular progress on Recent runs ([d357f3e](https://github.com/rubeniskov/bananas/commit/d357f3e0f8cb5efbbb6dba96a274458c378c00a5))
* **cloud:** rclone runtime — actually run sync entries ([2efaf74](https://github.com/rubeniskov/bananas/commit/2efaf74cb57fefad03fb2b2e95fe333dec67c7b1))
* **dashboard:** hot-reload theme on stats.toml change, drop service restart ([278db38](https://github.com/rubeniskov/bananas/commit/278db38de284fc2c2fc34e5e78922dcbff080f75))
* **dashboard:** re-enable Mali-400 GPU acceleration via lima + femtovg ([c869134](https://github.com/rubeniskov/bananas/commit/c86913462eedd4820de69c50b40828617858bfdd))
* **dashboard:** wire up Mali-400 lima/femtovg GPU path ([1114f56](https://github.com/rubeniskov/bananas/commit/1114f56b01988de09b011e09be3f044d430b5b61))
* **exports:** manage nfs-server lifecycle from the UI + status badge ([7bbb7c3](https://github.com/rubeniskov/bananas/commit/7bbb7c3776263a5b206982925ae047e6c1a83b44))
* **fstab:** mkdir -p mountpoints + mount -a on save, reset-failed nfs-server on start ([657d752](https://github.com/rubeniskov/bananas/commit/657d75214a9e335fcd5c0becca3bcbba6bcd9fd9))
* **image:** blacklist brcmfmac to silence WiFi-firmware boot spam ([4007fd3](https://github.com/rubeniskov/bananas/commit/4007fd3fa453efecd32de7d51108b79e4e262c3b))
* **image:** boot splash via psplash + clean kernel cmdline ([105f460](https://github.com/rubeniskov/bananas/commit/105f460cd315c3c72336786333cd8f506f067b2d))
* **image:** grow rootfs on first MMC boot, skip on NFS ([bfd8c96](https://github.com/rubeniskov/bananas/commit/bfd8c96f08513acbb43de9ec03e8491769bddfb0))
* **image:** make ROOT_PASSWORD_HASH optional, ship "bananas" placeholder ([94c450d](https://github.com/rubeniskov/bananas/commit/94c450d89c85fe34a4b25509116ff310e3fa21e5))
* **image:** ship bananas-dashboard (Slint LCD app) ([a03a7f0](https://github.com/rubeniskov/bananas/commit/a03a7f06355a65408f178f18e44152bc8535bef2))
* **image:** swap partition on first boot + vm.swappiness=10 ([4828e51](https://github.com/rubeniskov/bananas/commit/4828e51fbaac17b40406363957f2f42de64dcd50))
* **image:** U-Boot splash screen on the LCD ([981b161](https://github.com/rubeniskov/bananas/commit/981b161017f72da4bc45bd76f70c3fbe6f2b4aa0))
* **mounts:** editable mountpoint + auto-mkdir for fresh paths ([da649a6](https://github.com/rubeniskov/bananas/commit/da649a699cb96b386006c374023373f662aef512))
* **release:** bake + ship a dd-able SD-card image on every release ([0bc84a0](https://github.com/rubeniskov/bananas/commit/0bc84a0c6b4be444fea2242d87bfe022dba46647))
* **server:** bananas-server + bananas-helper Rust workspace ([29bdf15](https://github.com/rubeniskov/bananas/commit/29bdf158070417fc8eb4776c78af8aba84141bb2))
* **server:** cross-compile to armv7 + ship via Yocto recipe ([b28891e](https://github.com/rubeniskov/bananas/commit/b28891e24fac6afa798ede59229005654ce82af9))
* **server:** structured exports UI with checkboxes + per-row delete ([bfe3cc7](https://github.com/rubeniskov/bananas/commit/bfe3cc7bdeb35b387466d77288631709f13c7aab))
* **server:** yocto recipe + systemd units (stub) for bananas-server ([cc1a0d3](https://github.com/rubeniskov/bananas/commit/cc1a0d308a933e610a4d2d83a857a203a9c381be))
* **settings:** parsed forms for Stats + Dashboard, full timezone list ([d673895](https://github.com/rubeniskov/bananas/commit/d6738956e6dfcc37557a9b334c58e86064917a74))
* **settings:** split dashboard.toml/system.toml + new /settings route ([737ec87](https://github.com/rubeniskov/bananas/commit/737ec87fa9efcbb6f2d223cba015ba3c15832339))
* **splash:** black-bg splash variants — keep both, ship the dark one ([54a6be2](https://github.com/rubeniskov/bananas/commit/54a6be2f0bdfc2ddcf5787fa328c50dc5eb5d503))
* **stats,ui:** show CPU clock instead of busy% on the live tile ([8e43ce4](https://github.com/rubeniskov/bananas/commit/8e43ce4d2f745a6a7cb792c5b10c4c82958cdb7c))
* **stats:** CPU + drivetemp temperature monitoring ([090d0cb](https://github.com/rubeniskov/bananas/commit/090d0cbd636118f19c2b1cd90f93bf0bb218d6c2))
* **stats:** live Unix-socket pub/sub; remove SQLite read amplification ([7a81074](https://github.com/rubeniskov/bananas/commit/7a810746a6ad8ec4cb05c8075b255cf1daf85ed3))
* **ui:** block UI mutations during config-restore (busy overlay) ([d9547cb](https://github.com/rubeniskov/bananas/commit/d9547cb1f62d4c0b86788bbe7a35f4d03029afef))
* **ui:** form-based stats config + ship default + fix mounts row ([fdd5921](https://github.com/rubeniskov/bananas/commit/fdd59218e9604d6b9ad42f3771c066c39f129d1b))
* **ui:** in-app ConfirmModal + auto-poll cron progress ([9425e27](https://github.com/rubeniskov/bananas/commit/9425e27c1134529280038ed6cbe333a2d66e5ea1))
* **ui:** inline CPU temp in tile, restructure disk cards, provider badges ([abcf82b](https://github.com/rubeniskov/bananas/commit/abcf82bf1d7db3d75f211be57896a44a760d6a82))
* **ui:** light/dark theme with browser-preference default + picker ([c4908da](https://github.com/rubeniskov/bananas/commit/c4908da8c73e0da385c19fda4dc4543f5771c53c)), closes [ccc/#888](https://github.com/ccc/bananas/issues/888) [#666](https://github.com/rubeniskov/bananas/issues/666) [#555](https://github.com/rubeniskov/bananas/issues/555) [#f4f4f4](https://github.com/rubeniskov/bananas/issues/f4f4f4)
* **ui:** restore Stats config modal + polished form styling ([b0ff8c0](https://github.com/rubeniskov/bananas/commit/b0ff8c09a1c37b856519ddd00df8fe9ad88e6483))
* **ui:** show-protected toggle, copy-preview, stats-config modal ([f4e936d](https://github.com/rubeniskov/bananas/commit/f4e936d38113d53e2307962080edcf8db32aa7fc))
* web admin UI, persistent stats, privileged helper ops, fstab+perms+users editor ([cd0d85b](https://github.com/rubeniskov/bananas/commit/cd0d85b4fea49a4066275f8dd3f3e60fc957e5e6))
* **web,helper:** reboot button + SMART on right band; fix Poky MOTD ([a438f42](https://github.com/rubeniskov/bananas/commit/a438f428489ae87aa0038763fb1c16e11d6799a4))
* yocto-built BanaNAS image for Banana Pro (BPI-M1+) ([de811f6](https://github.com/rubeniskov/bananas/commit/de811f6fa6e0abfdaef7a60ef65d5080996cc5db))


### 🐛 Bug Fixes

* **auth:** let password_expired pass through helper redaction ([f2257d3](https://github.com/rubeniskov/bananas/commit/f2257d36dc4a5bbba5bfe578ace837df1c060976))
* **ci:** bump pixi to v0.66.0 to match local toolchain ([0ce42b2](https://github.com/rubeniskov/bananas/commit/0ce42b22dfb7c70afca9578b78fb6a77ac99ac1f))
* **ci:** drop apparmor user-namespace restriction so bitbake fakeroot works ([e43ee51](https://github.com/rubeniskov/bananas/commit/e43ee510d6118e12c88f7288d433c0c424f1e3f6))
* **ci:** drop maximize-build-space's build-mount-path so pixi cache fits ([b802e26](https://github.com/rubeniskov/bananas/commit/b802e26a50f794e2fb1aebde7868788b886996d0))
* **ci:** install armv7 rust target in the cross-compile job ([edf0685](https://github.com/rubeniskov/bananas/commit/edf06856edaaadec711ade128078437740023622))
* **ci:** stage rclone before the Yocto bake in release.yml ([0053395](https://github.com/rubeniskov/bananas/commit/00533950e4be8696d676e623ed9eda17a7e7bdb7))
* **ci:** swap maximize-build-space for free-disk-space ([e7d064c](https://github.com/rubeniskov/bananas/commit/e7d064cc3e463e928cd3afd7043c330ba72f8f8e))
* **cloud:** elevate rclone stats to NOTICE so progress bar goes determinate ([4d3cde7](https://github.com/rubeniskov/bananas/commit/4d3cde7322918ea3d7914df6200c3ebfc9fa8109))
* **cloud:** give a clear error when the sync's local_path is missing ([c0e7c32](https://github.com/rubeniskov/bananas/commit/c0e7c32bfa35ee3759e76325bb0572eae00afe4a)), closes [#1](https://github.com/rubeniskov/bananas/issues/1)
* **cloud:** migrate google_drive → drive on cloud.toml load ([d1c1307](https://github.com/rubeniskov/bananas/commit/d1c130795e507c0eaedddd29031f6e5a69c33631)), closes [#1](https://github.com/rubeniskov/bananas/issues/1)
* **cloud:** parse rclone v1.68 progress lines (no Transferred: prefix) ([67803ec](https://github.com/rubeniskov/bananas/commit/67803ec0db983d0f203ec55096e6c9e76ca4ed71))
* **cloud:** use rclone's actual backend names as provider keys ([35d03d5](https://github.com/rubeniskov/bananas/commit/35d03d5c7f2b24a6ed9db2c6fc9dd449fc07aa4d))
* **dashboard:** correct dejavu font package name (ttf-dejavu-sans) ([3718624](https://github.com/rubeniskov/bananas/commit/3718624157bf77cbe4f3935164664d630ace89e9))
* **dashboard:** default SLINT_BACKEND to linuxkms-software ([a2a27f1](https://github.com/rubeniskov/bananas/commit/a2a27f14f01f10e1584a860d3bff325fb5031200))
* **dashboard:** drop apostrophe + escaped quote from DESCRIPTION ([e577203](https://github.com/rubeniskov/bananas/commit/e5772032d1de439604ab9e77e05924e0043cbfc9))
* **dashboard:** drop femtovg renderer to avoid libgbm/x11 dep chain ([b9907d7](https://github.com/rubeniskov/bananas/commit/b9907d714b10a906f2872f29e79bba3335c742be))
* **dashboard:** RDEPENDS uses unsuffixed package name (libdrm not libdrm2) ([733332f](https://github.com/rubeniskov/bananas/commit/733332ff4d7024eda08578625aa9e93a911f5554))
* **dashboard:** SLINT_BACKEND name + ship fonts so fontique can match ([cbfa76e](https://github.com/rubeniskov/bananas/commit/cbfa76ee61f8df52fe268654a8f8e72c458374c9))
* empty Type/Label columns — route lsblk through the helper ([e1cf0c8](https://github.com/rubeniskov/bananas/commit/e1cf0c86e85cc1307f8176ac6da73e53b87d2e72))
* **exports:** auto-inject fsid= when rootfs is NFS so iterate-loop exports work ([68fe02f](https://github.com/rubeniskov/bananas/commit/68fe02f0616fad68bc9b4d800464caafad8717e2))
* **exports:** mkdir -p export paths before exportfs to keep nfs-server happy ([2ca880e](https://github.com/rubeniskov/bananas/commit/2ca880ed2a0460adcb2a346b47ecc9e1931d8d6f))
* **image:** break firstboot-resize ordering cycle, scrub Poky MOTD ([af8b85c](https://github.com/rubeniskov/bananas/commit/af8b85c40ce4f56f1f9b1ba15d442ba2b0e7af49))
* **image:** force full mesa for libgbm; rename libdrm→libdrm2 in RDEPENDS ([c19a444](https://github.com/rubeniskov/bananas/commit/c19a44450a13e2bd7f71ba6062ab14279a6b1907))
* **image:** set use-mailine-graphics override BEFORE require sun7i.inc ([81cc348](https://github.com/rubeniskov/bananas/commit/81cc3480b3c22d006c557430f6b8f1331b134bf0))
* **nfs:** start nfs-statd alongside nfs-server (macOS clients need it) ([a51baf9](https://github.com/rubeniskov/bananas/commit/a51baf92a3a60bc38bba01176814ad4f9ccd85fc))
* **pixi:** build task fails fast on bake errors ([aea262a](https://github.com/rubeniskov/bananas/commit/aea262af67bab3661290bbbc346f2d939847ebd8))
* **psplash:** add Upstream-Status to color patch (Yocto QA) ([1620190](https://github.com/rubeniskov/bananas/commit/16201900ffd24dd6d7bea9649798c616bb5013be))
* **psplash:** use outsuffix=default so /usr/bin/psplash actually ships ([b8fd39c](https://github.com/rubeniskov/bananas/commit/b8fd39c7d10a6d2c04be087270a06e771c7863f0))
* **release:** use Yocto's actual rootfs.wic symlink name ([ef35fda](https://github.com/rubeniskov/bananas/commit/ef35fdadd67f9909429a10f9eeb9ea253feafdc9))
* **server,image:** commit Cargo.lock, NFS all_squash, drop dead code ([3778478](https://github.com/rubeniskov/bananas/commit/3778478fd3d2e6db7d3d1a94737a9e8aa095f2b9))
* **stats:** bounce bananas-dashboard too on stats.toml save ([0aebc0b](https://github.com/rubeniskov/bananas/commit/0aebc0bb31ae74df9c29eed59623bbc6460d46c2))
* **ui:** info panels readable in dark mode ([cf2fe00](https://github.com/rubeniskov/bananas/commit/cf2fe000ed49f06e0b621921d780d5a462b0d772)), closes [#ffeef0](https://github.com/rubeniskov/bananas/issues/ffeef0) [#e7f8](https://github.com/rubeniskov/bananas/issues/e7f8)
* **ui:** unclassed buttons (Cancel, etc.) get explicit color ([7d94af5](https://github.com/rubeniskov/bananas/commit/7d94af54baf73c272de9583a213974ea46eb9870))


### ⚡ Performance

* batch of project-wide build + runtime improvements ([aa08ac1](https://github.com/rubeniskov/bananas/commit/aa08ac118f4ba039921d28be4508111de9ea79be))
* opt-level=3+LTO release profile; throttle dashboard repaints to 2 Hz ([2d4f1ca](https://github.com/rubeniskov/bananas/commit/2d4f1ca3a192b526983fb56e01d23e48f318273f))
* **server:** cache /api/storage 30s + parallelize SMART calls ([a5223f0](https://github.com/rubeniskov/bananas/commit/a5223f0a1f556ecf27d12564b994b3b869cff54f))
* tier-2 batch — pre-serialize WS, auto cpu_count, split CI cache ([dab388e](https://github.com/rubeniskov/bananas/commit/dab388e92a0656b9bdb3d823b8a1c45337940e89))
* **yocto:** enable BB hash equivalence (BB_HASHSERVE=auto) ([027c4ec](https://github.com/rubeniskov/bananas/commit/027c4ecec257248498073a6cd9cc906381ccf880))


### 🧹 Refactoring

* rename crates/ui → crates/webadmin (bananas-ui → bananas-webadmin) ([534bf8f](https://github.com/rubeniskov/bananas/commit/534bf8ff7af72fb6dc6de6f5f07add85cca2d705))
* **server:** move crates under crates/{server,helper} ([ad8d13b](https://github.com/rubeniskov/bananas/commit/ad8d13b2a0ead66d4769b80c23ca7b2dbe48e1f2))


### 📚 Documentation

* comprehensive README, LICENSE (MIT), CONTRIBUTING, CI/CD pipeline ([0244fc4](https://github.com/rubeniskov/bananas/commit/0244fc4a0c57e55abc27ac4a64e1280dcdaebf2a))
* **readme:** badges, mermaid architecture, bigger logo, drop roadmap ([7b453c3](https://github.com/rubeniskov/bananas/commit/7b453c34883fbce3813279e34ebf274ea21b5fef))
* **readme:** one-liner pipe for the SD flash step ([9b7b039](https://github.com/rubeniskov/bananas/commit/9b7b039ff4913b19c3af856723e22f1dd87b7ba5))
