import { nativeE2eTestTargets } from "./native-e2e-targets.mjs";
import { runNativeE2eTargets } from "./native-e2e-runner.mjs";

const testTargets = process.argv.slice(2);

console.log("[native-watch] Running native E2E in visible watch mode.");
console.log("[native-watch] Rebuild first if the app window shows a localhost:1420 connection failure.");

const exitCode = await runNativeE2eTargets({
  requestedTargets: testTargets,
  defaultTargets: nativeE2eTestTargets(),
  env: {
    ...process.env,
    WARDIAN_NATIVE_SKIP_BUILD: "1",
    WARDIAN_E2E_WATCH: "1",
    WARDIAN_E2E_STEP_DELAY_MS: process.env.WARDIAN_E2E_STEP_DELAY_MS || "750",
    WARDIAN_E2E_WATCH_KEEP_OPEN: process.env.WARDIAN_E2E_WATCH_KEEP_OPEN || "1",
  },
});
process.exit(exitCode);
