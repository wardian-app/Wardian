import { createE2eConfig } from "./playwright.config";

/** Flagged workbench runs always start their own correctly-configured server. */
export default createE2eConfig({ workbench: true });
