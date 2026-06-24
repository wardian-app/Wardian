import { nativeE2eTestTargets } from "./native-e2e-targets.mjs";
import { runNativeE2eTargets } from "./native-e2e-runner.mjs";

const testTargets = process.argv.slice(2);

const exitCode = await runNativeE2eTargets({
  requestedTargets: testTargets,
  defaultTargets: nativeE2eTestTargets(),
});
process.exit(exitCode);
