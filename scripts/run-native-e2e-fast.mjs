import { nativeE2eTestTargets } from "./native-e2e-targets.mjs";
import { runNativeE2eTargets } from "./native-e2e-runner.mjs";

const testTargets = process.argv.slice(2);

const exitCode = await runNativeE2eTargets({
  requestedTargets: testTargets,
  defaultTargets: nativeE2eTestTargets(),
  env: {
    ...process.env,
    WARDIAN_NATIVE_SKIP_BUILD: "1",
  },
});
process.exit(exitCode);
