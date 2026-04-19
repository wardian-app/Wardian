# Changelog

All notable changes to Wardian will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries from `0.3.0` onward are generated automatically by release-please from Conventional Commits. Entries for `0.1.0` through `0.2.1` were backfilled from git history and are thematic summaries rather than exhaustive commit lists.

## [0.3.0](https://github.com/tangemicioglu/Wardian/compare/wardian-v0.2.1...wardian-v0.3.0) (2026-04-19)


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
