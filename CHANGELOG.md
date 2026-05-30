# Changelog

All notable changes to Wardian will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries from `0.3.0` onward are generated automatically by release-please from Conventional Commits. Entries for `0.1.0` through `0.2.1` were backfilled from git history and are thematic summaries rather than exhaustive commit lists.

## [0.3.9](https://github.com/wardian-app/Wardian/compare/v0.3.8...v0.3.9) (2026-05-30)


### Features

* **cli:** Workflow Engine v2 thin CLI — exec/runs/replay (sub-project 4) ([#441](https://github.com/wardian-app/Wardian/issues/441)) ([a24dd45](https://github.com/wardian-app/Wardian/commit/a24dd45399c495a5993fa324a9a14c1a2c70619f))
* **run-view:** Workflow Run View — observe mode (sub-project 3b) ([#438](https://github.com/wardian-app/Wardian/issues/438)) ([cde6a4e](https://github.com/wardian-app/Wardian/commit/cde6a4e88f11fad44192026c3027c102c581a282))
* **workflow-v2:** add invoker foundation ([#456](https://github.com/wardian-app/Wardian/issues/456)) ([5032e82](https://github.com/wardian-app/Wardian/commit/5032e8250e1de9b1b1d5fe9f8ea987b4756272b3))
* **workflow-v2:** live executor — drive real agents + run lifecycle (sub-project 5a) ([#448](https://github.com/wardian-app/Wardian/issues/448)) ([b5466df](https://github.com/wardian-app/Wardian/commit/b5466df5e311ec4fe48790d33adb495a637a093c))
* **workflow-v2:** schedule invoker (sub-project 6b) ([#457](https://github.com/wardian-app/Wardian/issues/457)) ([35e9e21](https://github.com/wardian-app/Wardian/commit/35e9e216ba14550cb6e2f3a2cd2644955506e1fa))
* **workflows:** unified Workflows view — edit/observe/run, collapse tabs (sub-project 5b) ([#450](https://github.com/wardian-app/Wardian/issues/450)) ([9d4a3fa](https://github.com/wardian-app/Wardian/commit/9d4a3fa952d45efc968d679b74bf272bf0ae28d3))


### Bug Fixes

* **ci:** use release please PR output for lock sync ([#445](https://github.com/wardian-app/Wardian/issues/445)) ([09d513a](https://github.com/wardian-app/Wardian/commit/09d513ac9ee508b4934d2d7d8eab22a2382dec75))
* **cli:** speed up worktree discovery ([#455](https://github.com/wardian-app/Wardian/issues/455)) ([4a8fe80](https://github.com/wardian-app/Wardian/commit/4a8fe8023e9ed682c1266c69a423001dd4b6abb4))


### Documentation

* add llms.txt index ([#449](https://github.com/wardian-app/Wardian/issues/449)) ([c262185](https://github.com/wardian-app/Wardian/commit/c262185cc32cfedbcad407c59b68e4f21e27ec21))

## [0.3.8](https://github.com/wardian-app/Wardian/compare/v0.3.7...v0.3.8) (2026-05-29)


### Features

* **builder:** Workflow Builder v2 — edit mode (sub-project 3a) ([#437](https://github.com/wardian-app/Wardian/issues/437)) ([2cbb0e7](https://github.com/wardian-app/Wardian/commit/2cbb0e7c0fcc7a37f6135d34d3018a9a0cc615f5))


### Bug Fixes

* **agent:** ignore stale status after clear ([#440](https://github.com/wardian-app/Wardian/issues/440)) ([0ab36b6](https://github.com/wardian-app/Wardian/commit/0ab36b626302492d7e7cffce1b8bf26891d86c11))
* **ci:** publish frontend Codecov coverage ([#433](https://github.com/wardian-app/Wardian/issues/433)) ([94e9be1](https://github.com/wardian-app/Wardian/commit/94e9be18aac7456d1098c40033e64a5159b4d5b6))
* **updater:** let Windows helper escape supervisor job ([#442](https://github.com/wardian-app/Wardian/issues/442)) ([259c7b3](https://github.com/wardian-app/Wardian/commit/259c7b3ed01c11685768de080c3490daa77c67f7))

## [0.3.7](https://github.com/wardian-app/Wardian/compare/v0.3.6...v0.3.7) (2026-05-29)


### Features

* **cli:** add provider-aware queued delivery ([#356](https://github.com/wardian-app/Wardian/issues/356)) ([f4bf0a6](https://github.com/wardian-app/Wardian/commit/f4bf0a677542d36ac1a1c0b8593cd334877cdf5e))
* **engine:** v2 durable engine + consolidate workflow/engine into wardian-core ([#429](https://github.com/wardian-app/Wardian/issues/429)) ([8be5d7f](https://github.com/wardian-app/Wardian/commit/8be5d7f354cd8028785a4401104da78fe644ed7a))
* **explorer:** open files in external editor ([#419](https://github.com/wardian-app/Wardian/issues/419)) ([6940bee](https://github.com/wardian-app/Wardian/commit/6940beea3e7eb2a9c062ac628bab4afa71174889))
* **graph:** add agent relationship graph view ([#348](https://github.com/wardian-app/Wardian/issues/348)) ([4ecc0ba](https://github.com/wardian-app/Wardian/commit/4ecc0bac242d765f864874253f8ba1764d1fc689))
* **grid:** add chat mode for agent cards ([#345](https://github.com/wardian-app/Wardian/issues/345)) ([34ab3f6](https://github.com/wardian-app/Wardian/commit/34ab3f6875f865fa7864ab1bc7284e592a5d87cc))
* **grid:** add chat-first card composition mode ([#410](https://github.com/wardian-app/Wardian/issues/410)) ([a8a21d9](https://github.com/wardian-app/Wardian/commit/a8a21d9db3ca7bca35b73127f1d613393d0b56f3))
* **providers:** add antigravity support ([#327](https://github.com/wardian-app/Wardian/issues/327)) ([38fc7a7](https://github.com/wardian-app/Wardian/commit/38fc7a73b421b80ffa34152e0d62a88f4ba89893))
* **queue:** add action-needed alerts and filters ([#383](https://github.com/wardian-app/Wardian/issues/383)) ([bb3e43e](https://github.com/wardian-app/Wardian/commit/bb3e43ea1b2661f17b648ef64e898b3157f85154))
* **release:** add OS package manager manifests ([#326](https://github.com/wardian-app/Wardian/issues/326)) ([376b469](https://github.com/wardian-app/Wardian/commit/376b469d5bff048dd4615e63675fda2fcd8b62ed))
* **remote:** add interactive terminal attach ([#398](https://github.com/wardian-app/Wardian/issues/398)) ([50050a3](https://github.com/wardian-app/Wardian/commit/50050a3dffd30295f8ead4115dc211cb5552c64f))
* **remote:** add local-control PWA v1 ([#343](https://github.com/wardian-app/Wardian/issues/343)) ([1bcb50a](https://github.com/wardian-app/Wardian/commit/1bcb50ac55d0be421e7bf6a1790e0f04385d9938))
* **remote:** default mobile agent detail to terminal ([#370](https://github.com/wardian-app/Wardian/issues/370)) ([35a358a](https://github.com/wardian-app/Wardian/commit/35a358ad73d71b25220be53e492a56ff25f77df3))
* **remote:** warn when setup is incomplete ([#402](https://github.com/wardian-app/Wardian/issues/402)) ([8b31057](https://github.com/wardian-app/Wardian/commit/8b3105798244503d8ab1a4aacbf17d7bdc05a751))
* **settings:** add global settings modal and sparse defaults ([#341](https://github.com/wardian-app/Wardian/issues/341)) ([aec4904](https://github.com/wardian-app/Wardian/commit/aec4904fb57585b94bf30a6bc9d2309f7558aea9))
* **settings:** add titlebar telemetry visibility ([#382](https://github.com/wardian-app/Wardian/issues/382)) ([8ba4f21](https://github.com/wardian-app/Wardian/commit/8ba4f2173e8dfae3cf18061689c92172b4c0a30c))
* **settings:** configure watchlist spawn position ([#366](https://github.com/wardian-app/Wardian/issues/366)) ([79696a1](https://github.com/wardian-app/Wardian/commit/79696a15efdc97e3eea49e85d1c221ab82d0f790))
* **terminal:** add conservative mac select-all shortcut ([#404](https://github.com/wardian-app/Wardian/issues/404)) ([e2689f5](https://github.com/wardian-app/Wardian/commit/e2689f59e27f6ee8e030091a75375fbac6297ae0))
* **watchlist:** collapse team members ([#359](https://github.com/wardian-app/Wardian/issues/359)) ([78420fa](https://github.com/wardian-app/Wardian/commit/78420fa89fb62bec941c142c3cd3f78c825a82ac))
* **workflow:** wardian-workflow library + node type registry (v2 sub-project 1) ([#426](https://github.com/wardian-app/Wardian/issues/426)) ([63fd078](https://github.com/wardian-app/Wardian/commit/63fd078fa753f74d398eb47d2a9dc7fe3eaec84b))


### Bug Fixes

* **ci:** isolate provider hydration test state ([#412](https://github.com/wardian-app/Wardian/issues/412)) ([e91a676](https://github.com/wardian-app/Wardian/commit/e91a6761dddfee588bdf6394c21ecfc9dc446989))
* **engine:** submit Codex prompts after echo ([#431](https://github.com/wardian-app/Wardian/issues/431)) ([458794b](https://github.com/wardian-app/Wardian/commit/458794b7938b55541f6eadac4a43b9ee0d744a51))
* **git:** repair sidebar commit and publish flows ([#416](https://github.com/wardian-app/Wardian/issues/416)) ([1fb1792](https://github.com/wardian-app/Wardian/commit/1fb1792fbbb5a3828fe27caf91a8a5c829b925d2))
* **mailbox:** validate real-provider delivery ([#414](https://github.com/wardian-app/Wardian/issues/414)) ([5ed5ab2](https://github.com/wardian-app/Wardian/commit/5ed5ab2bbeaa736525bb423da9a668b1238dd6d8))
* **providers:** prevent antigravity watcher from auto-resuming on clear ([#367](https://github.com/wardian-app/Wardian/issues/367)) ([779e438](https://github.com/wardian-app/Wardian/commit/779e4381ef21739cfe195dcf45ff6bac68f6b065))
* **queue:** use native action-needed alerts ([#422](https://github.com/wardian-app/Wardian/issues/422)) ([cec6b37](https://github.com/wardian-app/Wardian/commit/cec6b37a906922ed15a83317eff61f32d1ee60b2))
* **runtime:** claim control endpoint outside Tokio context ([#374](https://github.com/wardian-app/Wardian/issues/374)) ([41dcdec](https://github.com/wardian-app/Wardian/commit/41dcdececf56e69f5987d842038e2e759cfa69c0))
* **runtime:** harden reliability audit failure modes ([#352](https://github.com/wardian-app/Wardian/issues/352)) ([f6a9387](https://github.com/wardian-app/Wardian/commit/f6a93878caa1425ffc994b630e77b46acf535882))
* **runtime:** isolate debug app home ([#372](https://github.com/wardian-app/Wardian/issues/372)) ([675775a](https://github.com/wardian-app/Wardian/commit/675775a5748820f8ff549f76a0341c3e72d60758))
* **runtime:** serialize agent lifecycle operations ([#338](https://github.com/wardian-app/Wardian/issues/338)) ([d450c40](https://github.com/wardian-app/Wardian/commit/d450c404d06a5bba88718d22e099fd97edf3ce90))
* **terminal:** stabilize provider redraw scrollback ([#428](https://github.com/wardian-app/Wardian/issues/428)) ([8338801](https://github.com/wardian-app/Wardian/commit/8338801905f8c495f0702363335fcb2277987a4d))
* **terminal:** stabilize resize rendering ([#389](https://github.com/wardian-app/Wardian/issues/389)) ([ddefcc6](https://github.com/wardian-app/Wardian/commit/ddefcc65efdc36be89b3a138eb5fb208e771aa47))
* **terminal:** WebGL leak, clear-time PTY race, codex dedup, cursor style ([#357](https://github.com/wardian-app/Wardian/issues/357)) ([d3023ed](https://github.com/wardian-app/Wardian/commit/d3023ed7dbe762579520665561296e25c02bf5f8))
* **ui:** capitalize roster provider labels ([#340](https://github.com/wardian-app/Wardian/issues/340)) ([efd2b4a](https://github.com/wardian-app/Wardian/commit/efd2b4a244a238a0b104b021d245b16f797af164))
* **ui:** compact sidebar titles ([#380](https://github.com/wardian-app/Wardian/issues/380)) ([461a1e9](https://github.com/wardian-app/Wardian/commit/461a1e94e834ec77074115f6e4cd32e296d54fae))
* **ui:** fit small-screen double grid ([#385](https://github.com/wardian-app/Wardian/issues/385)) ([2435190](https://github.com/wardian-app/Wardian/commit/2435190f63cacebb96c2796e7545c7554e868a65))
* **worktrees:** sync Wardian worktrees with Git registry ([#417](https://github.com/wardian-app/Wardian/issues/417)) ([f2cca29](https://github.com/wardian-app/Wardian/commit/f2cca298bce487688a818f73653fa79305c079ab))


### Documentation

* **remote:** expand Tailscale setup guide ([#388](https://github.com/wardian-app/Wardian/issues/388)) ([d747289](https://github.com/wardian-app/Wardian/commit/d747289890d58e438767253b16235ac51a802293))

## [0.3.6](https://github.com/tangemicioglu/Wardian/compare/v0.3.5...v0.3.6) (2026-05-19)


### Features

* **agents:** add copy full agent command ([#262](https://github.com/tangemicioglu/Wardian/issues/262)) ([4776ece](https://github.com/tangemicioglu/Wardian/commit/4776ece9502a95ccea90f4f2f69204c6b13eaf6e))
* **cli:** add command send mode ([#253](https://github.com/tangemicioglu/Wardian/issues/253)) ([1d7b66f](https://github.com/tangemicioglu/Wardian/commit/1d7b66fb3922eb9998fe32e122f492b869dcd7f1))
* **cli:** add structured ask replies ([#257](https://github.com/tangemicioglu/Wardian/issues/257)) ([63841ef](https://github.com/tangemicioglu/Wardian/commit/63841efad998cba8f26083afcc80691f4a0a5e35))
* **onboarding:** add contextual help entry points ([#294](https://github.com/tangemicioglu/Wardian/issues/294)) ([1df9c36](https://github.com/tangemicioglu/Wardian/commit/1df9c36e5dcb36fa54ef747d82a5bbe1e1d95ef4))
* **providers:** gate unavailable provider runtimes ([#311](https://github.com/tangemicioglu/Wardian/issues/311)) ([123553f](https://github.com/tangemicioglu/Wardian/commit/123553fea1b5adbe0d171e73e39bec47b683ebc6))
* **settings:** add guarded in-app updater ([#313](https://github.com/tangemicioglu/Wardian/issues/313)) ([fa8ea91](https://github.com/tangemicioglu/Wardian/commit/fa8ea9156099b77f1fd75ffa4364f064648858e7))


### Bug Fixes

* **agents:** hide provider resume ids ([#289](https://github.com/tangemicioglu/Wardian/issues/289)) ([8a9ac6e](https://github.com/tangemicioglu/Wardian/commit/8a9ac6e2c8fc9ca5cd86a7d76e9b95cd0820850b))
* **cli:** detect current Codex ready prompt ([#305](https://github.com/tangemicioglu/Wardian/issues/305)) ([414c8b9](https://github.com/tangemicioglu/Wardian/commit/414c8b9d9854b785e5a097a792aa4ae9d60a853e))
* **deps:** resolve dependabot vulnerabilities ([#316](https://github.com/tangemicioglu/Wardian/issues/316)) ([eed52d7](https://github.com/tangemicioglu/Wardian/commit/eed52d75072086c5bc471135a24155c22429f296))
* **gemini:** align manual session lifecycle ([#267](https://github.com/tangemicioglu/Wardian/issues/267)) ([45a6f97](https://github.com/tangemicioglu/Wardian/commit/45a6f972ef1b46be34ac2c4caee8d91af76ab64e))
* **gemini:** handle empty restarts and patch paths ([#309](https://github.com/tangemicioglu/Wardian/issues/309)) ([bcee066](https://github.com/tangemicioglu/Wardian/commit/bcee0668cfd1f99cc0d3a523f13fbe211b818f72))
* **gemini:** resume restored manual sessions ([#301](https://github.com/tangemicioglu/Wardian/issues/301)) ([cfc1a28](https://github.com/tangemicioglu/Wardian/commit/cfc1a28c0d14691a271562d8f94714c61a1104e9))
* **release:** sync Rust workspace versions ([#261](https://github.com/tangemicioglu/Wardian/issues/261)) ([241f06e](https://github.com/tangemicioglu/Wardian/commit/241f06eb53f93b6f200328aef16c8f4b6b559b08))
* **runtime:** harden agent update telemetry ([#303](https://github.com/tangemicioglu/Wardian/issues/303)) ([4770f3f](https://github.com/tangemicioglu/Wardian/commit/4770f3f44e105aed9f26b929426c2dd8d60dcb34))
* **source-control:** show active worktree name ([#265](https://github.com/tangemicioglu/Wardian/issues/265)) ([0cd5e18](https://github.com/tangemicioglu/Wardian/commit/0cd5e18433857ab92136344f7f30a6aa8c88bd64))
* **terminal:** harden real-provider PTY rendering ([#271](https://github.com/tangemicioglu/Wardian/issues/271)) ([20e5926](https://github.com/tangemicioglu/Wardian/commit/20e5926634d9aa46ba922d41bda08b2a2091aefe))
* **ui:** simplify help links and grid resizing ([#318](https://github.com/tangemicioglu/Wardian/issues/318)) ([59c83bd](https://github.com/tangemicioglu/Wardian/commit/59c83bd8eabac31e9ce0cceccd0032b198cb59ab))
* **ui:** tighten three-column shell density ([#285](https://github.com/tangemicioglu/Wardian/issues/285)) ([d8bb927](https://github.com/tangemicioglu/Wardian/commit/d8bb9278df0f7c05f6d5acfff27c6f33886ff11b))


### Documentation

* add first-run onboarding guide ([#293](https://github.com/tangemicioglu/Wardian/issues/293)) ([7a0c8c7](https://github.com/tangemicioglu/Wardian/commit/7a0c8c7da56edff7482137dfdb4b601f702c5728))
* add first-run troubleshooting guide ([#297](https://github.com/tangemicioglu/Wardian/issues/297)) ([64c68da](https://github.com/tangemicioglu/Wardian/commit/64c68da00d108730af678d65d10f3256756f27d3))
* add provider readiness guide ([#296](https://github.com/tangemicioglu/Wardian/issues/296)) ([48495ac](https://github.com/tangemicioglu/Wardian/commit/48495ac065b958769c197e2e2a4330ab196a0dc9))
* define docs maintenance workflow ([#299](https://github.com/tangemicioglu/Wardian/issues/299)) ([aa21521](https://github.com/tangemicioglu/Wardian/commit/aa21521c970afbed2da1494c89f78ed6f742a1c7))
* organize task-oriented feature guides ([#295](https://github.com/tangemicioglu/Wardian/issues/295)) ([3878fe1](https://github.com/tangemicioglu/Wardian/commit/3878fe1817331aa6db0a585d8d21b5592edbbc32))
* **providers:** standardize supported provider docs ([#291](https://github.com/tangemicioglu/Wardian/issues/291)) ([9d7688a](https://github.com/tangemicioglu/Wardian/commit/9d7688a9e833c622e4b75c8544b5de6c8274a5f8))
* publish VitePress documentation site ([#286](https://github.com/tangemicioglu/Wardian/issues/286)) ([885ddd0](https://github.com/tangemicioglu/Wardian/commit/885ddd0e154378c677808077bbfe46f38964b515))
* **readme:** refresh demo walkthrough gif ([#251](https://github.com/tangemicioglu/Wardian/issues/251)) ([84e4cd5](https://github.com/tangemicioglu/Wardian/commit/84e4cd578983870901a184e83780300ac3208155))
* refresh provider runtime references ([#307](https://github.com/tangemicioglu/Wardian/issues/307)) ([b365e51](https://github.com/tangemicioglu/Wardian/commit/b365e516010fac1de1f3c073644520a6df7b341b))
* **research:** add org control plane references ([#259](https://github.com/tangemicioglu/Wardian/issues/259)) ([28f35cc](https://github.com/tangemicioglu/Wardian/commit/28f35cc6bed642070af2606d7c62a9bceb07fb52))

## [0.3.5](https://github.com/tangemicioglu/Wardian/compare/v0.3.4...v0.3.5) (2026-05-12)


### Features

* **agent:** add custom clone flow ([#220](https://github.com/tangemicioglu/Wardian/issues/220)) ([3589c2d](https://github.com/tangemicioglu/Wardian/commit/3589c2d760d916811b6928f3c5117f915f659d05))
* **cli:** add ask command for one-turn agent queries ([#215](https://github.com/tangemicioglu/Wardian/issues/215)) ([55cd1e8](https://github.com/tangemicioglu/Wardian/commit/55cd1e8a576c7cdcbe35bf88447a1d591b9ceccc))
* **cli:** add worktree and watchlist parity slice ([#242](https://github.com/tangemicioglu/Wardian/issues/242)) ([daa6e18](https://github.com/tangemicioglu/Wardian/commit/daa6e18eb5decf60b12a21a500b6dca6479578e2))
* **worktrees:** support shared agent worktrees ([#223](https://github.com/tangemicioglu/Wardian/issues/223)) ([263f5b9](https://github.com/tangemicioglu/Wardian/commit/263f5b991132444c35f7bbd506445c9b0645f2a0))


### Bug Fixes

* **agent-status:** gate live status transitions ([#236](https://github.com/tangemicioglu/Wardian/issues/236)) ([6aad196](https://github.com/tangemicioglu/Wardian/commit/6aad1965d9c992b48b517030b9ee0ae6f6e5baed))
* **agents:** keep cloned agents in source team ([#225](https://github.com/tangemicioglu/Wardian/issues/225)) ([9b19b77](https://github.com/tangemicioglu/Wardian/commit/9b19b77d443ebf158713a1a97c9aec4d625191e5))
* **codex:** share Windows sandbox support ([#238](https://github.com/tangemicioglu/Wardian/issues/238)) ([5bd6e80](https://github.com/tangemicioglu/Wardian/commit/5bd6e80a006d61bece9711990d980c30d7491dc3))
* **control:** stabilize provider agent communication ([#222](https://github.com/tangemicioglu/Wardian/issues/222)) ([032faf4](https://github.com/tangemicioglu/Wardian/commit/032faf4a9c2202be661a79780457d6fb3030fd3b))
* **queue:** prevent overflow cards from shrinking ([#227](https://github.com/tangemicioglu/Wardian/issues/227)) ([9946f11](https://github.com/tangemicioglu/Wardian/commit/9946f11ea519b823281b2496b10cf647f74dd728))
* **release:** publish unified installers ([#206](https://github.com/tangemicioglu/Wardian/issues/206)) ([2c297ef](https://github.com/tangemicioglu/Wardian/commit/2c297efda9848447e9dbf04d270b2eba4d7b3f35))
* **watchlist:** preserve Claude last queried on launch ([#247](https://github.com/tangemicioglu/Wardian/issues/247)) ([f327e22](https://github.com/tangemicioglu/Wardian/commit/f327e22be017a566539ae47ef8172297b834461b))
* **workflows:** detect live agent turn completion ([#230](https://github.com/tangemicioglu/Wardian/issues/230)) ([4231116](https://github.com/tangemicioglu/Wardian/commit/423111633cd26607fe6288c8eaa77e4fed8a2cbc))


### Documentation

* add core feature screenshots ([#233](https://github.com/tangemicioglu/Wardian/issues/233)) ([a5450f2](https://github.com/tangemicioglu/Wardian/commit/a5450f21bc1c9411cd0cd9786b56fd23f1d05795))
* highlight CLI, Queue, and workflows ([#249](https://github.com/tangemicioglu/Wardian/issues/249)) ([bd6f98e](https://github.com/tangemicioglu/Wardian/commit/bd6f98e9492b945e375073c44acde4a7613c0d51))
* **research:** add roadmap reference maps ([#240](https://github.com/tangemicioglu/Wardian/issues/240)) ([042326c](https://github.com/tangemicioglu/Wardian/commit/042326c38388c21cc4fd46dac052f1e0e5eacfa0))
* specify custom agent clone ([86d9f60](https://github.com/tangemicioglu/Wardian/commit/86d9f60e3d8865b0f1790b4ae065a092281084b8))

## [0.3.4](https://github.com/tangemicioglu/Wardian/compare/v0.3.3...v0.3.4) (2026-05-04)


### Features

* **agents:** order new sessions predictably ([#204](https://github.com/tangemicioglu/Wardian/issues/204)) ([3c91f24](https://github.com/tangemicioglu/Wardian/commit/3c91f24bb1757476fb28eac0a7aec44379a52896))
* **cli:** implement Wardian agent command ([#193](https://github.com/tangemicioglu/Wardian/issues/193)) ([c2f1303](https://github.com/tangemicioglu/Wardian/commit/c2f1303cc022bd498f97eeea676c47bd95e5972a))
* **library:** refresh skills while Wardian is online ([#190](https://github.com/tangemicioglu/Wardian/issues/190)) ([319eb25](https://github.com/tangemicioglu/Wardian/commit/319eb25b20f6cb99bff79559132a34addf40cdbc))
* **settings:** add terminal font controls ([ffbc953](https://github.com/tangemicioglu/Wardian/commit/ffbc95387e3f6eae5f0c2ce19ed75d212bd2942b))
* **settings:** add terminal font controls ([8ae8ef2](https://github.com/tangemicioglu/Wardian/commit/8ae8ef24dab646f37c54ca25b77e8c7a3f3c524b))
* **sidebar:** add terminal navigation item ([#195](https://github.com/tangemicioglu/Wardian/issues/195)) ([8f02066](https://github.com/tangemicioglu/Wardian/commit/8f0206670ee6eb0e1f8573bba2bd59c31e933bd4))
* **spawn:** generate names for blank agents ([#197](https://github.com/tangemicioglu/Wardian/issues/197)) ([7bd89d7](https://github.com/tangemicioglu/Wardian/commit/7bd89d7feb977ffee861f5b230857fe6657e45c8))
* **terminal:** add standalone user terminal panel ([#198](https://github.com/tangemicioglu/Wardian/issues/198)) ([1079b31](https://github.com/tangemicioglu/Wardian/commit/1079b31ed66c5639eef331bbe8b2bd18fd6ee1dc))


### Bug Fixes

* **runtime:** repair macos build after spawn enablement ([a0cd827](https://github.com/tangemicioglu/Wardian/commit/a0cd827b9db16fcc27fb93909eec1e3bea9b0d35))
* **runtime:** repair macOS build after spawn enablement ([2aec49d](https://github.com/tangemicioglu/Wardian/commit/2aec49d32313c9cded445b447e7ebb778b5b3745))
* **terminal:** refine scroll behavior for TUI agents ([#188](https://github.com/tangemicioglu/Wardian/issues/188)) ([1b70b86](https://github.com/tangemicioglu/Wardian/commit/1b70b863b5cd6c48c0e368e71fef1edd8b5d258d))
* **terminal:** select agent on terminal focus ([#189](https://github.com/tangemicioglu/Wardian/issues/189)) ([aba5ce3](https://github.com/tangemicioglu/Wardian/commit/aba5ce3f5d24414934f0705530dcec724ba7db96))


### Documentation

* add README status badges ([4d80542](https://github.com/tangemicioglu/Wardian/commit/4d8054261878a554420ddfd7d779c11e3ce6b5b0))
* add README status badges ([f664700](https://github.com/tangemicioglu/Wardian/commit/f664700bbc6746c1263cfb8de8d58eb09c19a776))

## [0.3.3](https://github.com/tangemicioglu/Wardian/compare/v0.3.2...v0.3.3) (2026-05-01)


### Bug Fixes

* **agent:** enable spawn on non-windows ([ce0ed2a](https://github.com/tangemicioglu/Wardian/commit/ce0ed2a18d65d667fa57a5395004f11cdbc7d898))
* **agent:** enable spawn on non-Windows ([f716168](https://github.com/tangemicioglu/Wardian/commit/f71616822fdab3cf181abf9d309669b33d159386))
* **release:** resolve draft releases for backfill ([135554a](https://github.com/tangemicioglu/Wardian/commit/135554a00ac7e3338f3ee031d98e8c5aaaf33048))
* **release:** resolve draft releases for manual backfill ([edf21a9](https://github.com/tangemicioglu/Wardian/commit/edf21a9e82a96a430cc3b4f51c5695f91d85635a))
* **runtime:** close non-windows compatibility gaps ([ced70af](https://github.com/tangemicioglu/Wardian/commit/ced70afa235e9d28f381ea89b05f815aa004fc91))
* **runtime:** close non-Windows compatibility gaps ([0c4a3c4](https://github.com/tangemicioglu/Wardian/commit/0c4a3c465a1152095b81c5ea6f5a81485ea9eddd))

## [0.3.2](https://github.com/tangemicioglu/Wardian/compare/v0.3.1...v0.3.2) (2026-04-30)


### Features

* **agent:** add backend clone command ([47b3a99](https://github.com/tangemicioglu/Wardian/commit/47b3a99d59a45398ea1eb955c83422b190df3895))
* **agent:** add single-agent clone actions ([ada7fe9](https://github.com/tangemicioglu/Wardian/commit/ada7fe95e9be780ee3d436736ced7ad439b9949d))
* **agent:** expose clone in context menu ([f47df1e](https://github.com/tangemicioglu/Wardian/commit/f47df1ebb7a4098bc9f0e16513b03cc6b6a48089))
* **agents:** add workspace folder picker ([ee23a44](https://github.com/tangemicioglu/Wardian/commit/ee23a4491195cc5961e5a6936be558a51f6a5867))
* **agents:** add workspace folder picker ([37f4415](https://github.com/tangemicioglu/Wardian/commit/37f4415a52db2f8942e51d4d46d272da65dbbdcd))


### Bug Fixes

* **agent:** harden profile clone behavior ([848b539](https://github.com/tangemicioglu/Wardian/commit/848b539de75390b54343ea7b80b3f7f2ff9bf2d5))
* **git:** restore source control status ([8f1caa2](https://github.com/tangemicioglu/Wardian/commit/8f1caa268875daa66f4c7effe04f1ebca14b117d))
* **git:** restore source control status ([94df3cc](https://github.com/tangemicioglu/Wardian/commit/94df3cca35a91a9c7085a21ca4d546f36661e42d))
* **library:** list linked deployed skills ([fb526fc](https://github.com/tangemicioglu/Wardian/commit/fb526fcdbc14fddb8565247ad21e6a8f4b6f193a))
* **library:** list linked deployed skills ([94fb233](https://github.com/tangemicioglu/Wardian/commit/94fb233db1335a407f4fe7fa7f179dca9f6b4e3b))
* **release:** backfill tauri assets from release drafts ([5110d0f](https://github.com/tangemicioglu/Wardian/commit/5110d0fcd1975c06c6a446a6dd7017ccc37240f7))
* **release:** force tags for draft releases ([ad32cf2](https://github.com/tangemicioglu/Wardian/commit/ad32cf22c3fa7433e31a2abe05e36f5ceaec6697))
* **release:** force tags for draft releases ([736147d](https://github.com/tangemicioglu/Wardian/commit/736147d85f0cc1cd4eacca0f999385ebf5923fdb))
* **release:** upload assets from release drafts ([b5368fe](https://github.com/tangemicioglu/Wardian/commit/b5368fe9417d6262d7ac1e2f3aed9a40be40be45))
* **runtime:** import macos path helper for headless builds ([69c401a](https://github.com/tangemicioglu/Wardian/commit/69c401a254df6e0c304941355f09c3433a06be5d))
* **watchlist:** preserve last queried on relaunch ([b9a6145](https://github.com/tangemicioglu/Wardian/commit/b9a6145092de2b8aaf76799a967a5ac9b609f713))
* **watchlist:** preserve last queried on relaunch ([3512a4f](https://github.com/tangemicioglu/Wardian/commit/3512a4f4decabba96e619be8dc659859a82c50ba))
* **workflows:** open run modal in main view ([6a32ae1](https://github.com/tangemicioglu/Wardian/commit/6a32ae17482dd6917aa4a38a527a251f9477c89f))


### Documentation

* **agent:** plan clone implementation ([c626d1d](https://github.com/tangemicioglu/Wardian/commit/c626d1dd4d45d5bb5df6f1811c98de8577dad80e))
* **agent:** specify clone menu behavior ([3d4376e](https://github.com/tangemicioglu/Wardian/commit/3d4376e9c0479861fbeabbbc195d1c0886a3a467))

## [0.3.1](https://github.com/tangemicioglu/Wardian/compare/v0.3.0...v0.3.1) (2026-04-27)


### Features

* **agents:** implement unique identity and SQLite status tracking ([0ffc996](https://github.com/tangemicioglu/Wardian/commit/0ffc996d7ba1474bd58cd2231ebd941a6c42f2fd))
* **agents:** robust identity rotation and non-destructive clear ([f962f69](https://github.com/tangemicioglu/Wardian/commit/f962f6937f369511934b84f218ff85c29637c078))
* **grid:** honor gridStacked mode with exit button ([9e7d37f](https://github.com/tangemicioglu/Wardian/commit/9e7d37f114d810541717852e10777725b96e6f3c))
* **grid:** live multi-column preview during stack-exit drag ([09c4a9c](https://github.com/tangemicioglu/Wardian/commit/09c4a9c2b0a58aae6bf8edf0e31a89e8bd67a07e))
* **grid:** trigger stacked mode when drag snaps to full width ([eba167f](https://github.com/tangemicioglu/Wardian/commit/eba167f5e2c4c17b170870d06a4fdef549ce7a03))
* **layout:** add SidebarResizeHandle component ([6ad2433](https://github.com/tangemicioglu/Wardian/commit/6ad24336da98968ed7d76130ee23d7d16258ebbd))
* **layout:** extend useLayoutStore with sidebar widths and gridStacked ([bf008a3](https://github.com/tangemicioglu/Wardian/commit/bf008a35ffb51693c8d7e113e33a04d76c300574))
* **layout:** make left sidebar resizable ([976bd24](https://github.com/tangemicioglu/Wardian/commit/976bd2463282a4866c91830d59d10389e47e233e))
* **layout:** make right sidebar (AgentWatchlist) resizable ([d49bdd0](https://github.com/tangemicioglu/Wardian/commit/d49bdd04eddf46f7e9264334250e83f846bad589))
* **layout:** wire useLayoutStore sidebar widths to CSS variables ([2764e7c](https://github.com/tangemicioglu/Wardian/commit/2764e7cde147650614be90d7eb335e50a3ad7b46))
* **testing:** implement Spec 025 — coverage reporting and screenshot docs ([3cd331b](https://github.com/tangemicioglu/Wardian/commit/3cd331ba350911d3e0ec6048d8397a08f4308124))
* **watchlist:** flatten sorted team members ([10cd057](https://github.com/tangemicioglu/Wardian/commit/10cd057fae361770073941e5c47e38edb6a1b41a))


### Bug Fixes

* address PR review feedback ([4e61ea6](https://github.com/tangemicioglu/Wardian/commit/4e61ea6dd44be9fe59c3c7d5e9f3e7d1ed9e8d5d))
* **agents:** Build freeze ([841cb69](https://github.com/tangemicioglu/Wardian/commit/841cb6938d1e37e0ee6136fc3a6fd2f86c9d189c))
* **build:** switch Vite minifier to terser to unbreak xterm.js in release ([4b10d0b](https://github.com/tangemicioglu/Wardian/commit/4b10d0b6df42a9b066e175f4f6c2bd1fd7693a61))
* **ci/security/docs:** CI coverage install, security patches, Linux stable ([70cc008](https://github.com/tangemicioglu/Wardian/commit/70cc00814b4715d00a92b60c0cec99322e96fc9f))
* **ci:** unblock coverage install and linux check ([2b1fb24](https://github.com/tangemicioglu/Wardian/commit/2b1fb2406b3a45a5ee045c77014095fc948262bb))
* **ci:** unblock coverage install and linux check ([2d99094](https://github.com/tangemicioglu/Wardian/commit/2d990941efe1aa537344cd56a9b6aa05da248b33))
* **claude:** prevent stuck 'Processing' status by recognizing result events ([4b8be7c](https://github.com/tangemicioglu/Wardian/commit/4b8be7cdf79b59b35869019bbd6215dd2213a78e))
* **codex:** resolve provider session identity mismatch and state recovery ([d45b36d](https://github.com/tangemicioglu/Wardian/commit/d45b36dab96613d8650a2c8cfbf932a94cca859c))
* dedupe app and agent telemetry ([116eaef](https://github.com/tangemicioglu/Wardian/commit/116eaefe6d0edb6a93887d6dcad89f98d26645f0))
* **deps:** explicitly add @testing-library/dom peer dep to unblock unit tests ([9b16a22](https://github.com/tangemicioglu/Wardian/commit/9b16a221f56fa1159771b05e594ac8047dbd37b4))
* **dev:** prevent stale Vite module caching ([fc93e47](https://github.com/tangemicioglu/Wardian/commit/fc93e4711e1230a8214dff861d3a985c1bfb221f))
* **e2e:** remove generic screenshot workflow ([0fa7004](https://github.com/tangemicioglu/Wardian/commit/0fa70044ccbc034c673f61c7e883b252172d4f39))
* **e2e:** resolve screenshots path in ESM ([e224ab6](https://github.com/tangemicioglu/Wardian/commit/e224ab6946df75890180e9dc7e8e84e41b1d7e0e))
* **fs:** minor explorer open hotfix ([90c1487](https://github.com/tangemicioglu/Wardian/commit/90c1487a95a4b869d17bf60952be99f2a9b49cc6))
* **gemini/codex:** session identity, state recovery, and status tracking ([bd6bbbd](https://github.com/tangemicioglu/Wardian/commit/bd6bbbd7691d8c1da68b6d1fc7a92ae9ae9e0af4))
* **gemini:** bootstrap fresh session on clear and fix Processing status ([ff93df8](https://github.com/tangemicioglu/Wardian/commit/ff93df8d79f26ec3934ff0fc32f7d29da999b090))
* **gemini:** resolve session clear and action needed state synchronization ([24a4bae](https://github.com/tangemicioglu/Wardian/commit/24a4bae52dd40f52f50e1a8544cc22cf8d2fe58c))
* **gemini:** resolve session ID mismatch and state recovery ([89b79d5](https://github.com/tangemicioglu/Wardian/commit/89b79d568fbd7428b5c863459670d12d67074c1e))
* **gemini:** resolve session identity tracking and action needed detection ([00f75a6](https://github.com/tangemicioglu/Wardian/commit/00f75a68a0930313d45586b65620255cdaaba9f5))
* **grid:** correct maximized layout, add background menu, and fix scroll whitespace ([d053424](https://github.com/tangemicioglu/Wardian/commit/d053424aae59404e3f96c9bdfbce2a6d83b596b8))
* **grid:** switch maximized agent from watchlist ([5c75bf2](https://github.com/tangemicioglu/Wardian/commit/5c75bf2c05fe5e8ec48b145ffd143eb0c003ec56))
* **layout:** address PR [#130](https://github.com/tangemicioglu/Wardian/issues/130) review comments ([39c9f08](https://github.com/tangemicioglu/Wardian/commit/39c9f08cc03a10927b84aa0c0b4b59f0a3e5494f))
* **layout:** resizable sidebars and forced stacked grid mode ([d5c1354](https://github.com/tangemicioglu/Wardian/commit/d5c1354518012abdf94cb6a138e3b1b7724dfcb0))
* **library:** link deployed skills to source ([a91f8ae](https://github.com/tangemicioglu/Wardian/commit/a91f8ae8799c0ac28649da5e817817c81411262f))
* **library:** link deployed skills to source ([7284f18](https://github.com/tangemicioglu/Wardian/commit/7284f18be3407b4b2b9f3a0b45d0b8358b98f93b))
* normalize agent telemetry metrics ([d5d1933](https://github.com/tangemicioglu/Wardian/commit/d5d193378b2f4354e742866f1613603629c29671))
* **opencode:** prefer native windows binaries over shims ([ae239a1](https://github.com/tangemicioglu/Wardian/commit/ae239a1fdec043fb27452757f54c189879eb84fe))
* **opencode:** preserve saved theme in runtime config ([f0f6f09](https://github.com/tangemicioglu/Wardian/commit/f0f6f09ceea21ae12535420154ac363de6c2eb41))
* **opencode:** sync theme and status telemetry ([8958876](https://github.com/tangemicioglu/Wardian/commit/8958876dd276ac578ec61df67343337fb5ce3a08))
* **opencode:** wrap Windows shim launches ([87792b7](https://github.com/tangemicioglu/Wardian/commit/87792b79db647aa75828354bf9d5a2c0d00b67ff))
* resolve clippy warnings and terminal reset UX ([8c4ab67](https://github.com/tangemicioglu/Wardian/commit/8c4ab679ff39ad258c852156ab95968828931b00))
* robust agent identity, SQLite status tracking, and state recovery ([a5c3500](https://github.com/tangemicioglu/Wardian/commit/a5c35004a32993ffc3c68012f6fce0071d4c41f3))
* **runtime:** supervise Windows agent process trees ([12d8af3](https://github.com/tangemicioglu/Wardian/commit/12d8af3fbd9facc100bd4583df8bca11a2269744))
* **runtime:** supervise Windows agent process trees ([b1a27c5](https://github.com/tangemicioglu/Wardian/commit/b1a27c58f4a09c4cce65031d39632ae5d01c3244))
* **security:** patch postcss XSS and rand unsoundness (Dependabot [#3](https://github.com/tangemicioglu/Wardian/issues/3), [#4](https://github.com/tangemicioglu/Wardian/issues/4), [#5](https://github.com/tangemicioglu/Wardian/issues/5)) ([3aa85ae](https://github.com/tangemicioglu/Wardian/commit/3aa85aeb6c2d41257d4a4cb5f3b4b00fb01f87e3))
* **telemetry:** dedupe app and agent process metrics ([2209f8c](https://github.com/tangemicioglu/Wardian/commit/2209f8c8656c1beecbe8cc97c61d15ea2ffdd17f))
* **telemetry:** normalize agent resource metrics ([5787c7f](https://github.com/tangemicioglu/Wardian/commit/5787c7f92d99d8d28a0680b728b2c1106734e8b4))
* **terminal:** propagate Wardian theme changes to OpenCode ([df47f5d](https://github.com/tangemicioglu/Wardian/commit/df47f5db75dc9f9734c66b29ce25df141b13d3c4))
* **terminal:** restore Codex enter submission ([c8f322c](https://github.com/tangemicioglu/Wardian/commit/c8f322cc1495f37c1d5e95d61495e539ef4d5a03))
* **terminal:** translate codex enter into ESC+CR submit chord ([9264d80](https://github.com/tangemicioglu/Wardian/commit/9264d80a3d3990c21e4b7cf79cc1f1291cc2a7ac))
* **watchlist:** keep team drops aligned in grid ([c4e1cac](https://github.com/tangemicioglu/Wardian/commit/c4e1cace94a8cbe8e133cb456ffd7069c90a2c5f))
* **watchlist:** use one confirm for bulk delete ([2faaa6e](https://github.com/tangemicioglu/Wardian/commit/2faaa6e16a1b278b9b0470c4d20f28f3a93fd503))
* **workflows:** surface failures and optionalize timeouts ([f73065b](https://github.com/tangemicioglu/Wardian/commit/f73065baae0ed3b4232c017e13eed40a74c57288))
* **workflows:** use local timezone for scheduled trigger timestamps ([30d8092](https://github.com/tangemicioglu/Wardian/commit/30d809247a555ce18ccdbabbd9576619485bba6d))


### Documentation

* create implementation plan for provider status and resume alignment ([0b99495](https://github.com/tangemicioglu/Wardian/commit/0b99495a32092e97aea96584c470a562bba5b955))
* **plans:** add implementation plan for responsive terminal width ([ada005c](https://github.com/tangemicioglu/Wardian/commit/ada005c1f47e1ea60f0c309ceb1ff56c6abf6fab))
* promote Linux to stable support ([9fecd9c](https://github.com/tangemicioglu/Wardian/commit/9fecd9c084fffc20ed1f72e877ec3d040e5a8481))
* **readme:** restructure intro, add demo gif and early-development notice ([b4e24fa](https://github.com/tangemicioglu/Wardian/commit/b4e24fab2713ada3af82807fa17210065bd450bb))
* record bulk delete and opencode windows behavior ([9945c27](https://github.com/tangemicioglu/Wardian/commit/9945c272a1df2b587600216f77203710c65c754c))
* replace personal absolute path in real-provider e2e example ([efacbcb](https://github.com/tangemicioglu/Wardian/commit/efacbcb5b55f61d5f0417ba1379432de2a23ea1e))
* **specs:** add 022 responsive terminal width design ([c9be3ea](https://github.com/tangemicioglu/Wardian/commit/c9be3ea0192a9bcbb9eea7e8b33ee4f5d2039915))
* **specs:** add spec 024 for wardian CLI and agent command ([ad217c1](https://github.com/tangemicioglu/Wardian/commit/ad217c1a1a68f4ab1929473943c35d152b369999))
* **specs:** correct 022 to extend existing useLayoutStore ([f2129f4](https://github.com/tangemicioglu/Wardian/commit/f2129f43cdaec94ea7cb62c8ebcfcb27b2d52166))

## [0.3.0](https://github.com/tangemicioglu/Wardian/compare/v0.2.1...v0.3.0) (2026-04-20)


### Features

* add Generalist agent class as default and remove Designer class ([9fdfb10](https://github.com/tangemicioglu/Wardian/commit/9fdfb10c2c202a63f86bfa86fe72332c861ba663))
* Add GitHub Actions CI workflow for security audits, frontend quality checks, and backend testing/compilation across Windows and Linux. ([fc67bad](https://github.com/tangemicioglu/Wardian/commit/fc67bad8d35997fb8d61fc46b6bb332e336fe7df))
* add provider-agnostic issue templates ([6f9c6b9](https://github.com/tangemicioglu/Wardian/commit/6f9c6b9b0aa7f4eeefd51551996d9721bc862841))
* **agents:** add clear session action ([fab666d](https://github.com/tangemicioglu/Wardian/commit/fab666d4cb6f4acf7e473c2a727cca1ffd5fd55f))
* **agents:** add last modified ([a29efdf](https://github.com/tangemicioglu/Wardian/commit/a29efdf0523073824ad5beff1256745989ea3915))
* **backend:** centralize path resolution and improve provider discovery for multi-platform support ([bdfe883](https://github.com/tangemicioglu/Wardian/commit/bdfe883a3c266e6631ba5c969491721f30a9ed13))
* **branding:** replace logo with simplified variants and restore SVG assets ([5104fe7](https://github.com/tangemicioglu/Wardian/commit/5104fe766fa91f498add8495e96ae9a4c6c5672a))
* **classes:** migrate to bulk 'Reset All Prompts' functionality ([4100bea](https://github.com/tangemicioglu/Wardian/commit/4100beaa77aa355341e590dc1f279709aa88995e))
* **commands:** implement copy to clipboard for quick prompts ([#103](https://github.com/tangemicioglu/Wardian/issues/103)) ([4e294d3](https://github.com/tangemicioglu/Wardian/commit/4e294d3e2405f8add117ec335968e6807dd0daf7))
* **commands:** implement copy to clipboard for quick prompts ([#103](https://github.com/tangemicioglu/Wardian/issues/103)) ([48e97cd](https://github.com/tangemicioglu/Wardian/commit/48e97cdfb33972574cc8569b54d8f80ef81816a5))
* **engine:** add role_mappings to WorkflowDefinition for role-based agent dispatch ([f5d227d](https://github.com/tangemicioglu/Wardian/commit/f5d227da2225d63f702f704e8f4a4dd029f10fb9))
* **engine:** add TurnCompleted event to AgentEvent for reliable workflow turn detection ([8ac0a03](https://github.com/tangemicioglu/Wardian/commit/8ac0a03f59b074b61e5c5ecdf6e081a1acf887a3))
* **engine:** auto-migrate existing workflows to role-based agent mapping ([a2b45ed](https://github.com/tangemicioglu/Wardian/commit/a2b45ed7f86be7ad45f5a0a990613c04d8638a91))
* **engine:** replace cron with calendar-style scheduler, add role-based agent mapping ([d2dd628](https://github.com/tangemicioglu/Wardian/commit/d2dd628cc4e4425c3776cbdc7426a6cbdbced672))
* **engine:** resolve agent roles from workflow role_mappings with agent_id fallback ([c83a988](https://github.com/tangemicioglu/Wardian/commit/c83a988191d276322630470a27ee6a77f188f76b))
* **explorer:** add local filesystem open action ([99f9284](https://github.com/tangemicioglu/Wardian/commit/99f92848a004870a38f60ad6c1e82ab19bb5fefd))
* **explorer:** enhance ui layout, readable folders, and resolve agent paths ([faa41c1](https://github.com/tangemicioglu/Wardian/commit/faa41c11bb7fac823c95398a4fb67404c4a31808))
* **explorer:** git file color coding in explorer view ([3f6ed93](https://github.com/tangemicioglu/Wardian/commit/3f6ed9333e72df5ce1e15466b0439a514742990b))
* **explorer:** implement File Explorer Sidebar Tab ([3e8b43e](https://github.com/tangemicioglu/Wardian/commit/3e8b43e40f2e868ebca473e56abe66854ed9519d))
* filter Dashboard and Grid views by active watchlist ([236092b](https://github.com/tangemicioglu/Wardian/commit/236092b023bc96e582c1505c0b7aacf06871e662))
* **git:** add commit history to source control panel ([1fa2c57](https://github.com/tangemicioglu/Wardian/commit/1fa2c57285b39ae2f31fc1fcf223aabb354c04af))
* **git:** add source control panel with worktree action UX ([34d682d](https://github.com/tangemicioglu/Wardian/commit/34d682d544048984a9b62673a0dffdf97ec83772))
* **grid:** implement synchronized track-based grid with tactile resizing and flush layout ([0b5ea2b](https://github.com/tangemicioglu/Wardian/commit/0b5ea2b4b50ec02f2918818c497245583eb1045f))
* **grid:** refine gutter visuals and fix track calculation logic ([4260343](https://github.com/tangemicioglu/Wardian/commit/4260343babcd5dd264032f1ac971a5b3d0544655))
* **grid:** Synchronized Grid Layout with Tactile Resizing ([9bdacd7](https://github.com/tangemicioglu/Wardian/commit/9bdacd7d118d587f4d9b66b9f9ae712c3e6afb05))
* implement autonomous shell and script workflow nodes with IO separation and security validation ([c131f56](https://github.com/tangemicioglu/Wardian/commit/c131f565acb8ac5c8f97143923207bcfd972e564))
* implement core workflow engine, execution state types, and scheduled run management system ([65a3ab9](https://github.com/tangemicioglu/Wardian/commit/65a3ab9ccf82f6f9d5e2bd204b44601e7b23675d))
* implement light mode, terminal theming, and transparent logo ([73b2ead](https://github.com/tangemicioglu/Wardian/commit/73b2eadd8593900d3bbb1be60283c6787e2883d6)), closes [#46](https://github.com/tangemicioglu/Wardian/issues/46)
* implement Loop node with cyclic execution, UI validation, and run safety ([8ab39b5](https://github.com/tangemicioglu/Wardian/commit/8ab39b59fca80075467f0119950a94c350710595))
* implement mouse-based drag-and-drop and unified selection in main views; fix terminal interaction issues ([c62417a](https://github.com/tangemicioglu/Wardian/commit/c62417a0ea4adb08a758313422b7b2696665de04))
* implement UI placeholders for main stage and sidebar views ([9bd440b](https://github.com/tangemicioglu/Wardian/commit/9bd440b3e21464830561161d85ea28085eec6ead))
* implement workflow builder mockup version ([811ecfc](https://github.com/tangemicioglu/Wardian/commit/811ecfc69e237009ea0d03d5458249974006190b))
* improve Codex provider, workflow engine, and agent terminal infrastructure ([7c65319](https://github.com/tangemicioglu/Wardian/commit/7c65319168f98e9fe19e1f383612a48c0ab38cd9))
* improve workflow engine, script error detection, and canvas centering. Closes [#37](https://github.com/tangemicioglu/Wardian/issues/37) ([f45fc52](https://github.com/tangemicioglu/Wardian/commit/f45fc529722b2489e33927a889442c908082f1ee))
* Initial commit - Wardian v0.1.0 ([7b7c1d5](https://github.com/tangemicioglu/Wardian/commit/7b7c1d56d1d9ae7cc59cd7b84d4fa0f0ac4bc639))
* **library:** add Assign Skill modal with target selection ([a0cbe56](https://github.com/tangemicioglu/Wardian/commit/a0cbe563e24ee0f41eeaf0ec591927d810fe8d48))
* **library:** add auto-patch functionality for gemini skills ([88747ec](https://github.com/tangemicioglu/Wardian/commit/88747ec7ea6d17b301f48785435451bb2e15945c))
* **library:** display and manage existing skill deployments ([ead174c](https://github.com/tangemicioglu/Wardian/commit/ead174c1fa06cb923ad82e28ee9b7684fe008021))
* **library:** implement filesystem-based skill deployment ([f638ceb](https://github.com/tangemicioglu/Wardian/commit/f638ceb5b603593ddfcfcbbf7c300af09fdd53c2))
* **library:** implement main library view and quick prompt injection ([f86e119](https://github.com/tangemicioglu/Wardian/commit/f86e119035324f44f5d6b4ece76991bd22f83773))
* **library:** improve command panel quick prompt UX ([3c86d8c](https://github.com/tangemicioglu/Wardian/commit/3c86d8c59b5160791dab67ba7a4b6639c6b3a529))
* **library:** improve command panel quick prompt UX ([f726b03](https://github.com/tangemicioglu/Wardian/commit/f726b03bee03c96b30e321d5d87c672108bebba1))
* **library:** improve library grid UX ([c628f72](https://github.com/tangemicioglu/Wardian/commit/c628f72edc3f543f80e4e02dcd6884f40411672b))
* **library:** refine UI terminology and add run actions ([830eeae](https://github.com/tangemicioglu/Wardian/commit/830eeae099a5a47cc33e1f841a8587b10e4388a0))
* **library:** unify default and custom class lists to share skill management UI ([462e12a](https://github.com/tangemicioglu/Wardian/commit/462e12a8bc4f7b91b63d3a3d6aecc3298afb5944))
* migrate agent data and configurations to ~/.wardian cross-platform convention ([bb4a322](https://github.com/tangemicioglu/Wardian/commit/bb4a3229492785fb0928c621b3389311056cb316))
* **migration:** add migrate_home_layout() with schema version tracking and copy-then-delete semantics ([d32193f](https://github.com/tangemicioglu/Wardian/commit/d32193f4a7dcbe96f1bfa5f2e2a70401b97f6526))
* **migration:** run migrate_home_layout() at startup before Tauri builder ([7bffe36](https://github.com/tangemicioglu/Wardian/commit/7bffe36065fda9cb4f5a05523bdbffc4e4b82bf6))
* **migration:** update shell_settings.json path to settings/shell.json ([afb9fe7](https://github.com/tangemicioglu/Wardian/commit/afb9fe76a9077dfcea3d6c28e3618a0c9d6d1e55))
* **migration:** update wardian_state.json path to settings/state.json ([c8b3d26](https://github.com/tangemicioglu/Wardian/commit/c8b3d2634cd0ceff1d2b1debba3c86ef618f9ff2))
* **migration:** update watchlist path to watchlists/index.json ([2aa7cdb](https://github.com/tangemicioglu/Wardian/commit/2aa7cdb376cb90f746ef6719c554fb0684562c5e))
* **migration:** update workflow_logs/ to logs/workflows/ and scheduled_runs.json to scheduled_workflows.json ([afe693e](https://github.com/tangemicioglu/Wardian/commit/afe693e80e5906d46a6c42c5d32ddfcc48625e46))
* **navigation:** remove redundant sidebar collapse arrows and centralize control in top bar, resolving github issue 83 ([625dcc9](https://github.com/tangemicioglu/Wardian/commit/625dcc95b865bdddab40b67110cc710c188fc59e))
* **opencode:** implement status detection and query count tracking ([8c8b1db](https://github.com/tangemicioglu/Wardian/commit/8c8b1dbd7eaab430346a48b9f1abde8cde6b7fac))
* **prompts:** upgrade and generalize default agent classes ([0d36ec6](https://github.com/tangemicioglu/Wardian/commit/0d36ec6d54114e9bceec0362bb46906cbe44bc6a))
* **prompts:** upgrade and generalize default agent classes ([6906f77](https://github.com/tangemicioglu/Wardian/commit/6906f77101ddd574274ebe1ab234bdfb3af719bc))
* **providers:** add codex habitat and status support ([c22824f](https://github.com/tangemicioglu/Wardian/commit/c22824fbbde0fa6b4327801d5066363d9f828ce2))
* **providers:** add codex support and stabilize agent status ([56aa47a](https://github.com/tangemicioglu/Wardian/commit/56aa47aa59a404dcffecf35ac9e47e117645289f))
* **providers:** add OpenCode and harden terminal runtime ([5a52541](https://github.com/tangemicioglu/Wardian/commit/5a525418360cb0b8651dfbd38098619b8424471f))
* **providers:** add opencode support ([aa7d6ca](https://github.com/tangemicioglu/Wardian/commit/aa7d6ca53b857bb3f89adbdd75528486ae48a746))
* **providers:** complete Claude Code integration with status tracking and advanced settings ([44f40c7](https://github.com/tangemicioglu/Wardian/commit/44f40c7a00a3202908de49435402a42b4899a60b))
* **providers:** implement generic AgentProvider architecture resolving [#64](https://github.com/tangemicioglu/Wardian/issues/64) ([b1df0ec](https://github.com/tangemicioglu/Wardian/commit/b1df0ec4dbb3b97156d3a1f42bd67275467a87f3))
* release v0.1.2 - Refactor agent panels, standardize ListEditor UI, and implement path validation ([15d9a58](https://github.com/tangemicioglu/Wardian/commit/15d9a589faee9eb57b8e48241dbd93e5409533a7))
* **release:** add automated multi-OS release pipeline ([01b5cda](https://github.com/tangemicioglu/Wardian/commit/01b5cdab38f17dadbee08083207d11ce570b0eda))
* **runtime:** add unified shell selection ([5a1d92e](https://github.com/tangemicioglu/Wardian/commit/5a1d92e464bb546355541cc245e72e128bb760f6))
* **runtime:** add unified shell selection ([61082ba](https://github.com/tangemicioglu/Wardian/commit/61082ba800e45d04f67cb3e1538a8c0d8d79488a))
* **scheduler:** add ScheduleDefinition and ScheduledRun data models ([e432afb](https://github.com/tangemicioglu/Wardian/commit/e432afba5af24c083a8932af3dbe1e638c22d028))
* **scheduler:** friendly schedule labels, delete button, and bridge cron triggers to scheduler ([527fc83](https://github.com/tangemicioglu/Wardian/commit/527fc83aebedf911c563ee1fc442bebf7b5588fb))
* **scheduler:** scheduled run persistence and Tauri CRUD commands ([d8cbca9](https://github.com/tangemicioglu/Wardian/commit/d8cbca971891316b61736b87b2a410e82e7572fe))
* **scheduler:** unified heartbeat scheduler reads scheduled_runs.json every 30s ([5c40598](https://github.com/tangemicioglu/Wardian/commit/5c40598d469f6f783309af98a212143c08b581a3))
* **session:** refine persistence policy and add source control panel ([196ea5a](https://github.com/tangemicioglu/Wardian/commit/196ea5a6028130c6271fe847a754089111a1d1aa))
* setup GitHub Actions CI and PR template ([5a03e71](https://github.com/tangemicioglu/Wardian/commit/5a03e714e817a423d7c6e71bfd3ab58e80d58f69))
* synchronize selection between watchlists and main Grid/Dashboard views ([a672e4e](https://github.com/tangemicioglu/Wardian/commit/a672e4e81c9354a7694b664182972f03d7ffb3f6))
* **testing:** add comprehensive E2E test infrastructure with 15 core feature tests ([904adff](https://github.com/tangemicioglu/Wardian/commit/904adff08d19c7cb17fef9e474d6b2adf9ed70b2))
* **testing:** add comprehensive E2E test infrastructure with 15 core feature tests ([7e5e7e5](https://github.com/tangemicioglu/Wardian/commit/7e5e7e5df43d41cc053f59113d15548592555adc))
* **testing:** add native tauri e2e harness ([d803389](https://github.com/tangemicioglu/Wardian/commit/d803389f6c6909ea9c39690224d39dbe7d2db4e7))
* **testing:** implement automated testing infrastructure with mock provider ([0106af2](https://github.com/tangemicioglu/Wardian/commit/0106af2f3d872114af1ae83821771c9be02de396))
* TradingView-style watchlist sidebar with mouse-based drag-drop, context menus, multi-list membership, and persistent storage ([d5948f1](https://github.com/tangemicioglu/Wardian/commit/d5948f1d31ff77da7f469622266c1d7db0c1cf0f))
* **ui:** abstract agent context menu and apply to grid view top bar ([ba2d75d](https://github.com/tangemicioglu/Wardian/commit/ba2d75dadebc063de959b9c1da1e06426825879b))
* **ui:** add custom styled scrollbars to dashboard and grid views ([46f08b3](https://github.com/tangemicioglu/Wardian/commit/46f08b369809a2413c24e553ca8bc884f5fdce46))
* **ui:** add macOS native traffic light window controls ([f82f29f](https://github.com/tangemicioglu/Wardian/commit/f82f29f721486fe8f31d7e37bff2ee4ac8717160))
* **ui:** refactor header layout, adjust tab sizing, and update sidebar icons ([a96d5a4](https://github.com/tangemicioglu/Wardian/commit/a96d5a4562ff87f496b62f10a2a004f855644a0f))
* **ui:** standardize semantic theming and refine color highlights ([bc68bca](https://github.com/tangemicioglu/Wardian/commit/bc68bca7a75408cca18d439b81b29c1b4c713160))
* Update agent Off state filtering, colors, and explicit status text ([ddf1561](https://github.com/tangemicioglu/Wardian/commit/ddf15615165d5dbd1a70ffe244909f5bd074b529))
* **watchlist:** add ColumnPicker popover component ([2ab9293](https://github.com/tangemicioglu/Wardian/commit/2ab9293232448c84802eea03e01c25df5e0ec68e))
* **watchlist:** add formatUptime, formatRelativeTime, cycleSort, sortAgents utilities ([6e4cfa0](https://github.com/tangemicioglu/Wardian/commit/6e4cfa020c50584028c60c86d969322a5b1e39b1))
* **watchlist:** add gear column picker, dynamic grid, sortable headers, optional column cells ([0e0a265](https://github.com/tangemicioglu/Wardian/commit/0e0a265beba53caf8d02d01448dca405f9032350))
* **watchlist:** add load/save commands for watchlist prefs and agent interactions ([3699bec](https://github.com/tangemicioglu/Wardian/commit/3699bec13e2fcf32d8a1237c17507819c9e0d68b))
* **watchlist:** add prefs and interactions state to App.tsx, record last-queried on sendCommand ([86b8534](https://github.com/tangemicioglu/Wardian/commit/86b853439e5ca71600b24886f445cb18088d5d87))
* **watchlist:** add WatchlistPrefs and AgentInteractions types ([50075d0](https://github.com/tangemicioglu/Wardian/commit/50075d0bb3bf02bce9a271f2f5bcd43577fca6a2))
* **watchlist:** customizable columns, sortable headers, FS restructure, session persistence ([61aa9a6](https://github.com/tangemicioglu/Wardian/commit/61aa9a6d49f03beef1b8698421d3305e4f015aac))
* **watchlist:** make Agent column sortable by name ([1093446](https://github.com/tangemicioglu/Wardian/commit/1093446b674beca80824622016d36bf7eec290ce))
* **watchlist:** make status/query-count toggleable, fix ColumnPicker background opacity ([fc3d0a6](https://github.com/tangemicioglu/Wardian/commit/fc3d0a61dfe9430f9344b21840d792456633d579))
* **watchlists:** add agent teams ([e37126b](https://github.com/tangemicioglu/Wardian/commit/e37126bcf822b743013eee6bfbf7e42bddef11b3))
* **watchlists:** add agent teams ([888253e](https://github.com/tangemicioglu/Wardian/commit/888253e41560089ee47a62fa750d440c2da59f55))
* **watchlist:** wire prefs and interactions into AgentWatchlist from App.tsx ([b059cd9](https://github.com/tangemicioglu/Wardian/commit/b059cd9a5e58c1b03dbb271a5e1c9f6e3325ff09))
* **workflow:** implement auto-save on run, neutral reset UI, and snappy centering ([eac37fb](https://github.com/tangemicioglu/Wardian/commit/eac37fba02af0c99016df4b8ac09eab45552d4a3))
* **workflow:** implement dynamic trigger and node parameters ([e8d6915](https://github.com/tangemicioglu/Wardian/commit/e8d691506d27a3ffb0e6dc95a088b2825645fcab))
* **workflow:** implement workflow sidebar UI and panic toggle backend connection ([279a705](https://github.com/tangemicioglu/Wardian/commit/279a705d4aede08872e87fcdbdb22a9805bc47bc))
* **workflows:** headless execution fixes, runtime agent roles, and sidebar improvements ([0f5828c](https://github.com/tangemicioglu/Wardian/commit/0f5828cae9da910bfd1a2ef346797952712f7afe))
* **workflows:** make session persistence explicit ([2464f72](https://github.com/tangemicioglu/Wardian/commit/2464f723ef2eedea93b755bf2d154488aa981313))
* **workflows:** redesign sidebar with reordering, governance controls, and unit tests ([d636517](https://github.com/tangemicioglu/Wardian/commit/d636517b6db9a7e09a2c2c064f11d44bf46ee203))
* **workflows:** upgrade scheduler engine with recurrence rules ([32f86b6](https://github.com/tangemicioglu/Wardian/commit/32f86b6466ccf33cf9b4d0a01f94fd9cac99933c))
* **workflows:** upgrade scheduler engine with recurrence rules and ui parity ([ae0ca05](https://github.com/tangemicioglu/Wardian/commit/ae0ca05a31385d1ede86ec6fb52cdac719b7c9c9))


### Bug Fixes

* address all Copilot review findings ([46441ac](https://github.com/tangemicioglu/Wardian/commit/46441acced82d9b08d42e4eabb69d27db62cdf79))
* **agent:** auto-create and inject private agent folder on spawn ([7b57296](https://github.com/tangemicioglu/Wardian/commit/7b57296857ac6200fd88a28d4d369ce32e0a685a))
* **agents:** add regular session persistence override ([5a45c00](https://github.com/tangemicioglu/Wardian/commit/5a45c0082c900b609e14212e6fe6d53e95885653))
* **agents:** automatically inject private agent folder into include paths ([40ffbc0](https://github.com/tangemicioglu/Wardian/commit/40ffbc0b2467ea3a8b082e55c5d6e168eaf3c1ab))
* **agents:** resolve gemini init failure and correct idle status polling ([3f78b0e](https://github.com/tangemicioglu/Wardian/commit/3f78b0e83b3cf4ae9a0bff703f49495ac37e72db))
* **agents:** resolve gemini init failure and correct idle status polling ([a689b31](https://github.com/tangemicioglu/Wardian/commit/a689b31d7c8ed179a34940c185a99a211ed816c3))
* **agents:** resolve lint warnings, fix UI state sync, and re-apply session fixes ([ce6ff91](https://github.com/tangemicioglu/Wardian/commit/ce6ff917f94a3b8275669317ddac3f7ae081845d))
* **agents:** restore session ID retrieval and fix Windows headless execution ([f450a0a](https://github.com/tangemicioglu/Wardian/commit/f450a0a3e8899904c29ed81c7e24a89cf4d3cfc4))
* **agents:** return interrupted sessions to idle ([9f06d9f](https://github.com/tangemicioglu/Wardian/commit/9f06d9f7e59f802f2b27de75f2e9bd96464367d6))
* **agents:** stabilize claude and codex status handling ([da965ce](https://github.com/tangemicioglu/Wardian/commit/da965ce6ef573844c29622360186ce6e926c99a4))
* **backend:** align session/resume IDs and fix binary resolution in agent spawning ([55ec892](https://github.com/tangemicioglu/Wardian/commit/55ec89224750bad0726ac5cf0cd5797ab1675f9c))
* **backend:** resolve agent spawn failures on macOS ([b2370e6](https://github.com/tangemicioglu/Wardian/commit/b2370e62aa3d1518ba877b907c678665acd4a27b))
* **backend:** robustly parse headless session ID from stdout ([1d8d373](https://github.com/tangemicioglu/Wardian/commit/1d8d3737212a468ea6386fc8847fc88d3363f71e))
* **backend:** wait for result event before resuming headless session ([80d91fd](https://github.com/tangemicioglu/Wardian/commit/80d91fdbe96b7d1c2c84cc8e65fafcc989f0e5ac))
* **build:** gate Windows-only deps behind cfg(windows) for macOS support ([41c5a85](https://github.com/tangemicioglu/Wardian/commit/41c5a858a02904a8233146be940713fe5b6c035f))
* change terminal maximization from duplicate component to CSS-based fullscreen to preserve scrollback and global map ([d10c184](https://github.com/tangemicioglu/Wardian/commit/d10c18439f139b7d5932936b79ae1edd8b936f5b))
* **ci:** drop libappindicator3-dev (Tauri 1) conflicting with ayatana ([e79509d](https://github.com/tangemicioglu/Wardian/commit/e79509d54afc99f1c669463581c83c4814e49701))
* **claude:** stabilize status detection ([7d7f1a3](https://github.com/tangemicioglu/Wardian/commit/7d7f1a37e6255ef254abf5fd85435a47df18af1f))
* **clippy:** collapse guarded match arms in manager ([944dc09](https://github.com/tangemicioglu/Wardian/commit/944dc091b9d9bc30006e6313c79296f1d50b4cb3))
* **clippy:** deref instead of clone on Copy type Option&lt;SystemTime&gt; ([bde0a6f](https://github.com/tangemicioglu/Wardian/commit/bde0a6f4504b407a6081ac2e457570ca854c4fe3))
* **clippy:** use &Path instead of &PathBuf in migration fn signatures ([21a2706](https://github.com/tangemicioglu/Wardian/commit/21a2706d657a2326c17fd2b6bb654702607160ce))
* **codex:** Fix codex prompt insertion ([82d0e0b](https://github.com/tangemicioglu/Wardian/commit/82d0e0b27867eeaa6ce362231ea0cce6891c636b))
* deduplicate query count, gate debug hook, fix idle threshold ([83bce69](https://github.com/tangemicioglu/Wardian/commit/83bce69891ebd02a59fe7b4dbe4c52cc0371dae4))
* **engine:** reliable agent node turn detection, provider-aware headless fallback, unified response flattening ([6c02663](https://github.com/tangemicioglu/Wardian/commit/6c02663d815929aee77505ae2b503278371812ae))
* **explorer:** use file explorer icon for root reveal ([84c4d92](https://github.com/tangemicioglu/Wardian/commit/84c4d92764f94fbbbb2be777f1ab29375eba5eab))
* **grid:** ensure resize gutters span full scroll height ([807d373](https://github.com/tangemicioglu/Wardian/commit/807d373c1176d292b76bad3ed21bb7a0b34c24d3))
* **grid:** FIx maximize behavior ([e843a1f](https://github.com/tangemicioglu/Wardian/commit/e843a1f24fd78e41dcbf120bdb1a14c77d6f0325))
* **grid:** improve scroll behavior and gutter visuals ([ef3d872](https://github.com/tangemicioglu/Wardian/commit/ef3d8727cd567cd7e8e7afa4d6b61360a2128a45))
* **grid:** remove flex-1 from GridView to prevent vertical gutter clipping ([ca7622b](https://github.com/tangemicioglu/Wardian/commit/ca7622b6b5f8640f3844219739006012251acdc1))
* **grid:** remove unused container variable in GridView tests ([bc0b58e](https://github.com/tangemicioglu/Wardian/commit/bc0b58e14210ce61a3f9ceea2d5af53e62165dc9))
* **grid:** resolve final linter error and update test expectations ([996b661](https://github.com/tangemicioglu/Wardian/commit/996b66114ad2e333895a1b7c316f0c3aa66936dd))
* **grid:** resolve linter errors and move reset grid to context menu ([54eb812](https://github.com/tangemicioglu/Wardian/commit/54eb812e4b4e7878f31159b9940095ddda24f066))
* **grid:** resolve linter errors in GridView and AgentWatchlist ([792f3bf](https://github.com/tangemicioglu/Wardian/commit/792f3bfcd2679df37cc9c2456f0efb17e7f011b6))
* **grid:** use absolute positioning for guide lines to support scrollable content ([2764772](https://github.com/tangemicioglu/Wardian/commit/2764772643385f1d6d7d0c13680b5735e41d856b))
* **habitat:** require provider session ids ([48344d6](https://github.com/tangemicioglu/Wardian/commit/48344d66e1a8f9e4048c6dcb34ae3f504a7ccf12))
* **library:** decouple prompt and skill state in library store ([1583b83](https://github.com/tangemicioglu/Wardian/commit/1583b83e5df6353512dd3d2d3409ba907529ec3e))
* **library:** resolve activeTab jumping and add Assign Prompt modal ([506462a](https://github.com/tangemicioglu/Wardian/commit/506462ade06c3ae4a1add850e6d194a4e7333ff3))
* **opencode:** restore TUI telemetry and session continuity ([c43613e](https://github.com/tangemicioglu/Wardian/commit/c43613ee43c5268dffa167ca8724153e48c3c797))
* **opencode:** restore TUI telemetry and session continuity ([119f8dc](https://github.com/tangemicioglu/Wardian/commit/119f8dc9125fa15516acfd16a0a6ad4534c5b930))
* **opencode:** stabilize tui runtime and prompt delivery ([86c6518](https://github.com/tangemicioglu/Wardian/commit/86c65184b06ece3e16c0569b554906e3ccfb695b))
* **opencode:** use project arg for interactive sessions ([5a0dac0](https://github.com/tangemicioglu/Wardian/commit/5a0dac0c2df1eb43d0012a6f5c98621147cecd22))
* **orchestration:** eliminate zombie subprocess leakage ([b08fe99](https://github.com/tangemicioglu/Wardian/commit/b08fe999b79f1195a71086a402e6d18101081c76))
* **providers:** address opencode review feedback ([db61fb9](https://github.com/tangemicioglu/Wardian/commit/db61fb9642b441db48622f2d29f4f34320e6d51a))
* **providers:** stabilize codex runtime and workflow ui ([b87be49](https://github.com/tangemicioglu/Wardian/commit/b87be491b152156faf4758b171513f24e1d3bdc3))
* **release:** strip package-name prefix from release tags ([3ac7d04](https://github.com/tangemicioglu/Wardian/commit/3ac7d048f22c2b545a4228f48d4fbbf3e453cfe6))
* **release:** strip package-name prefix from release tags ([9a6e37a](https://github.com/tangemicioglu/Wardian/commit/9a6e37a56e88d6754a8bbad39f168a5449e0f8bc))
* Remove provider bootstrap ([ca7149c](https://github.com/tangemicioglu/Wardian/commit/ca7149c53257c46d994979df548dee7bbbc8cb7a))
* restore query_count increment on OpenCode submit (single source) ([41e9503](https://github.com/tangemicioglu/Wardian/commit/41e95035b2ecdee69dbb80e58df3092e330df706))
* **review:** address grid maximize and workflow shell coverage ([911bebf](https://github.com/tangemicioglu/Wardian/commit/911bebfdee5ae34219f46bdf7a7ec8ad395a8eef))
* **review:** address PR feedback on git and settings flows ([daeb2c5](https://github.com/tangemicioglu/Wardian/commit/daeb2c578f36c8b98b6b2ad99a550379936884fd))
* **review:** address scheduler and status feedback ([d6ccf92](https://github.com/tangemicioglu/Wardian/commit/d6ccf92008ae4c9e7a372fe6471dd1d4eddd7f82))
* **runtime:** preserve provider args in git bash ([5607d39](https://github.com/tangemicioglu/Wardian/commit/5607d39000b927a2c2f1f17304040abb9f101182))
* **runtime:** resolve cross-platform compile issues ([e285ace](https://github.com/tangemicioglu/Wardian/commit/e285acec7650c2b8bd96e3a78efc8a066b69f2ed))
* **session:** refine resume behavior and settings UX ([617691f](https://github.com/tangemicioglu/Wardian/commit/617691f2f2050432b104bb90921daf73cf778332))
* **sidebar:** preload library and workflows ([95a4df2](https://github.com/tangemicioglu/Wardian/commit/95a4df28cdef6196f6602846488c17244dd34f94))
* terminal deadlock fixes - focus filter, event-based IO, output batching, smart scroll ([1248fbd](https://github.com/tangemicioglu/Wardian/commit/1248fbd0f0b69df62d3fe89069e2c62ca9bafe76))
* terminal freeze and corruption by gating resize/move behind window events and adding dimension guards ([70a3507](https://github.com/tangemicioglu/Wardian/commit/70a3507cc1d0b66f907063949d5ef05384e94ce5))
* **terminal:** harden provider rendering and scrollback ([532e494](https://github.com/tangemicioglu/Wardian/commit/532e494ae719fa6dcf9c61862c49d4e96f515bd0))
* **terminal:** resolve ConPTY deadlocks & maximize bugs, rename Coordinator ([ee1b929](https://github.com/tangemicioglu/Wardian/commit/ee1b9294fbd760eb4f9acf216341724a6ac0a78a))
* **test:** update test name from Command Center to Command ([dd1a522](https://github.com/tangemicioglu/Wardian/commit/dd1a522376fd99af468d124c319776f895113485))
* **titlebar:** toggle maximize button icon based on window state ([9071c2a](https://github.com/tangemicioglu/Wardian/commit/9071c2a12805cb7cb5e63e36cada5807a876aab4))
* **ui:** mathematically restructure 1080p center grid density to eliminate Gem cli 65-column TUI wrap clipping and restore v-scroll, maximize window on launch ([aa3730a](https://github.com/tangemicioglu/Wardian/commit/aa3730a8fc337a19ba95f38edc8caff1489dfb20))
* **ui:** replace window.confirm with custom centered dialog ([fe5ebc8](https://github.com/tangemicioglu/Wardian/commit/fe5ebc8a6c29e2bab6144b640d8cd9eb5ef49c0e))
* **ui:** unify agent status rendering and ensure reliable initialization timestamps ([c7235b2](https://github.com/tangemicioglu/Wardian/commit/c7235b2f1a8e85fd64a0bd01cb24f767f89e1fbc))
* **ui:** use startDragging() for titlebar drag on macOS ([53e4262](https://github.com/tangemicioglu/Wardian/commit/53e426220bd554e6e60e573e90e59a898acc4d36))
* update test for 6s idle threshold ([86bf860](https://github.com/tangemicioglu/Wardian/commit/86bf8602b4fc7d524a6d1ccd52024f6a7057bab8))
* Update watchlist behavior to work with new paused state and resolve [#36](https://github.com/tangemicioglu/Wardian/issues/36) ([b26432c](https://github.com/tangemicioglu/Wardian/commit/b26432c0a05d997a778dc1e2f6eec81f878d99e2))
* Update watchlist behavior to work with new paused state and resolve [#36](https://github.com/tangemicioglu/Wardian/issues/36) ([7e3a205](https://github.com/tangemicioglu/Wardian/commit/7e3a2056a8fb374bb4fcbcfd2a9d08a612d0232a))
* View switching keeps terminal history ([85c7d80](https://github.com/tangemicioglu/Wardian/commit/85c7d80018e35eacc1be200a7f29c10b3b181ed4))
* **watchlist:** clamp name column with minmax, reduce optional column widths ([13a7b16](https://github.com/tangemicioglu/Wardian/commit/13a7b16351273a707fe8a12bbe6abe1a27d90410))
* **watchlist:** merge loaded prefs with defaults so new columns always appear ([f112f48](https://github.com/tangemicioglu/Wardian/commit/f112f4819edfa95e54d3d1c17252cd8998bb8447))
* **watchlist:** replace arrow sort indicators with border cue, reclaim column width ([626ca14](https://github.com/tangemicioglu/Wardian/commit/626ca14b8dabec3da40a3eca42239fc1db188b76))
* **watchlists:** address agent team review feedback ([1abe52f](https://github.com/tangemicioglu/Wardian/commit/1abe52fe7002c67ba8220b9e6d59a0de484c27d1))
* **watchlist:** track last-queried via query_count delta, fix column width alignment ([da5d9e3](https://github.com/tangemicioglu/Wardian/commit/da5d9e3784c4ec2a71523797352e77c0b832bd36))
* **watchlist:** widen provider column to fit sort arrow ([134bfcf](https://github.com/tangemicioglu/Wardian/commit/134bfcf6d3fe2026614bc56f90efd2642ed6a01c))
* **windows:** hide background shell windows ([b63f757](https://github.com/tangemicioglu/Wardian/commit/b63f757ea52a6f678d02ac903d9b5f5274cf2aa2))
* **workflow:** restore input_schema logic and update block defaults ([0167688](https://github.com/tangemicioglu/Wardian/commit/016768860b3e9ed08e21e1ea0b9c6590773445eb))
* **workflows:** decouple scheduled run status by instance ([f4b219c](https://github.com/tangemicioglu/Wardian/commit/f4b219c237dfdb9a577fd6153936a37a26fe3b91))
* **workflows:** emit Tauri event upon background schedule deletion to avoid stale UI state ([4b579b2](https://github.com/tangemicioglu/Wardian/commit/4b579b2cb460a2d3157a8ccc0dcf616d4c4f66f5))
* **workflows:** isolate scheduled run status per instance ([5383a2b](https://github.com/tangemicioglu/Wardian/commit/5383a2b0d8ab189b7e17059add48acfcadb63f3d))
* **workflows:** stabilize scheduled monitoring ([acf5690](https://github.com/tangemicioglu/Wardian/commit/acf5690e49f7f4723b19698b88b46f90fcddc476))
* **workflows:** stabilize scheduler and runtime execution paths ([67a8df7](https://github.com/tangemicioglu/Wardian/commit/67a8df75c5512c5012695354ca4acd1ecc2f6a6c))


### Documentation

* add comprehensive documentation and update README with provider, platform, and feature details ([80826b1](https://github.com/tangemicioglu/Wardian/commit/80826b15c4949b6491ea4391a5597ac8ae414b15))
* add CONTRIBUTING.md and update agent instructions for PRs ([560e016](https://github.com/tangemicioglu/Wardian/commit/560e01677a2204044e5ed0408b1f5bd551e45120))
* add Download section to README ([a7a1f3f](https://github.com/tangemicioglu/Wardian/commit/a7a1f3f43f3b69fac022a600fea5d14c097fc0fc))
* add README, MIT license, update .gitignore ([16513cd](https://github.com/tangemicioglu/Wardian/commit/16513cd2949045b5fc0d8d20a9d511a6a8219da0))
* add Specs 005 and 006 for navigation and explorer view ([3ce9509](https://github.com/tangemicioglu/Wardian/commit/3ce95098a5d7ad0f42755c8b4ea6746fbe9379f4))
* **adrs:** update index for clarity and consistency ([d3b3428](https://github.com/tangemicioglu/Wardian/commit/d3b3428ad443143fd118bdcaef488dd6a1118050))
* **agents:** include all 10 default roles in roles.md ([9530eac](https://github.com/tangemicioglu/Wardian/commit/9530eac20165a3be533d421ed38fdea1795cb76f))
* **changelog:** backfill entries for 0.1.0 through 0.2.1 ([7e31373](https://github.com/tangemicioglu/Wardian/commit/7e3137329bd9d9c098f3927dddadde810460b979))
* **changelog:** backfill prior version entries + fix CI apt conflict ([adec661](https://github.com/tangemicioglu/Wardian/commit/adec661d9d447d1fa9e7fb26f1d2aff601fc541a))
* clean up readme and update feature list ([6e09094](https://github.com/tangemicioglu/Wardian/commit/6e09094d6b21eff178fb30c1508e3343a0dd7686))
* comprehensive update of user manual and guides ([8b92c62](https://github.com/tangemicioglu/Wardian/commit/8b92c62191c207d3b0776e15957cab5db1779c7d))
* expand user and developer documentation coverage ([fababd4](https://github.com/tangemicioglu/Wardian/commit/fababd449383f2065b2a448ad8d9ea555bc82a95))
* Fix outdated info ([436bc77](https://github.com/tangemicioglu/Wardian/commit/436bc7700547ae7e61ba6f11db6adbfad8dcc7b3))
* **gemini:** simplify pre-commit checklist and clarify naming conventions ([b436249](https://github.com/tangemicioglu/Wardian/commit/b4362497be9fda363b6c0eb10a4fc3b777e7e2ab))
* implement comprehensive architecture documentation, ADRs, and user guides ([3499823](https://github.com/tangemicioglu/Wardian/commit/3499823d59138e2dc20bc9ebbf42be1385210c9d))
* mark library system ADR as implemented ([9c4c13d](https://github.com/tangemicioglu/Wardian/commit/9c4c13dc38bcd331a4a0b240617e71e960be850c))
* **plans:** add release system implementation plan ([a78fafb](https://github.com/tangemicioglu/Wardian/commit/a78fafb9f44a0c2a3ce07995f964baf596437bc9))
* **providers:** add runtime integration notes ([72a90df](https://github.com/tangemicioglu/Wardian/commit/72a90df6f95875d6e7cd9db194b936b61c8f6b6a))
* remove documentation audit spec file ([2b1cc8b](https://github.com/tangemicioglu/Wardian/commit/2b1cc8b86fd7475af77f3f862dbcd92d3759484c))
* rename ADRs to Specs and update architectural guidelines ([19d08c4](https://github.com/tangemicioglu/Wardian/commit/19d08c4838c13ea187b56c5ee6a2f1a0b3ed4a53))
* seed CHANGELOG.md for release-please ([a48db99](https://github.com/tangemicioglu/Wardian/commit/a48db99698f0174177a5830c1f1176837383b3cb))
* specify agent teams and bulk actions ([a39bb35](https://github.com/tangemicioglu/Wardian/commit/a39bb356fb105a3678c113626736a6116283835b))
* **specs:** add release system design (021) ([f0b7239](https://github.com/tangemicioglu/Wardian/commit/f0b7239a40dcf393c1f1ae85db4568f8daf5512a))
* **specs:** add spec 018 for source control panel ([f6319a5](https://github.com/tangemicioglu/Wardian/commit/f6319a5774c99e1d506f9a5983485e7445244d66))
* **specs:** define workflow session persistence policy ([72b42f0](https://github.com/tangemicioglu/Wardian/commit/72b42f01e49521b303d26284cd9414fdb990917f))
* Update readme build instruction ([b9fccd1](https://github.com/tangemicioglu/Wardian/commit/b9fccd18a198e30bed521f5db21078868a290678))
* **workflows:** add workflow documentation hub ([9a44116](https://github.com/tangemicioglu/Wardian/commit/9a44116c6fa6684f0d3a51a921022521b11d8114))
* **workflows:** add workflow documentation hub ([707b264](https://github.com/tangemicioglu/Wardian/commit/707b264573fae9e417644f3e756d4b95e18def98))

## [0.2.1] - 2026-03-22

### Features

- **Skill Library**: filesystem-based skill deployment system with a main library view, quick prompt injection, and skill/prompt assignment modals across agents and classes.
- **Class Management**: unified default and custom class lists sharing a single skill management UI.
- **Command Panel**: refined quick-prompt UX and added run actions for library items.
- **Navigation**: centralized sidebar collapse control in the top bar; refactored header layout and tab sizing; updated sidebar icons.
- **Branding**: simplified logo variants and restored SVG asset pipeline.
- **Auto-patch**: Gemini-CLI skills are auto-patched on deployment for consistent behavior.
- **Workflow sidebar**: redesigned with reordering, governance controls, and unit test coverage.

### Bug Fixes

- Restored session-ID retrieval on agent spawn; fixed Windows headless execution.
- Aligned session and resume IDs; fixed binary resolution in agent spawning.
- Resolved lint warnings and UI state-sync issues across agents.
- Auto-created and injected the private agent folder on spawn, ensuring include paths are correct.
- Titlebar: toggle maximize-button icon based on window state; fixed layout collapse on resize.
- Library: resolved `activeTab` jumping; decoupled prompt and skill state in the library store.

### Documentation

- Comprehensive architecture docs, ADR backfill, and user guides landed.
- Renamed ADRs to Specs; updated index and architectural guidelines.

## [0.2.0] - 2026-03-13

### Features

- **Workflow Builder**: initial mockup and then full implementation of the workflow builder, including Loop nodes with cyclic execution, UI validation, run safety, and auto-save on run.
- **Autonomous Nodes**: shell and script workflow nodes with IO separation and security validation.
- **Grid & Dashboard**: mouse-based drag-and-drop with unified selection across main views; views now filter by the active watchlist with synchronized selection.
- **Theming**: light mode, terminal theming, transparent logo, and standardized semantic theming across the app.
- **Agent Classes**: added the Generalist agent class as the default; removed the Designer class.
- **Agent Menus**: aligned menus, added workspace path, and improved watchlist interactions.

### Bug Fixes

- View switching no longer discards terminal history.
- Script error detection hardened in the workflow engine.
- Canvas centering and reset UX improved.

### Refactoring

- Codebase modularized and layout architecture reorganized.
- Terminology standardized (Warden → Coordinator/Agent).

## [0.1.2] - 2026-03-07

### Features

- Agent-panel refactor, standardized `ListEditor` UI, and path validation for agent configurations.

## [0.1.1] - 2026-03-06

### Features

- **Watchlist**: TradingView-style sidebar with mouse-based drag-and-drop, context menus, multi-list membership, and persistent storage.
- **Agent state**: improved Off-state filtering, color semantics, and explicit status text.
- **UI placeholders**: initial main-stage and sidebar view scaffolding.
- **Cross-platform data**: agent data and configurations migrated to `~/.wardian`.

### Bug Fixes

- Terminal: resolved ConPTY deadlocks, maximize bugs, and focus-filter issues; moved to event-based IO with output batching and smart scroll; gated resize/move behind window events with dimension guards.
- Terminal maximization rewritten as CSS fullscreen to preserve scrollback and the global map.
- Watchlist behavior updated to work with the new paused state (closes #36).
- Grid density reworked for 1080p to eliminate 65-column TUI wrap clipping in Gem CLI; restore vertical scroll; maximize window on launch.

### Refactoring

- Renamed Warden → Coordinator (Agent); Workflows (UI) terminology adopted.

### Documentation

- Added README, MIT license, and updated `.gitignore`.

## [0.1.0] - 2026-02-24

Initial public commit. Wardian's first release: a Tauri-based integrated agent environment with a multi-agent grid, terminal panels, and basic orchestration primitives.
