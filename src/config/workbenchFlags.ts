export type WorkbenchDeveloperFlags = {
  workbench_enabled: boolean;
};

export type WorkbenchFlagEnvironment = Readonly<
  Record<string, string | boolean | undefined>
>;

/** Developer cutover stays opt-in until the Task 19 verification gates pass. */
export function resolveWorkbenchFlags(
  environment: WorkbenchFlagEnvironment,
): WorkbenchDeveloperFlags {
  return {
    workbench_enabled: environment.VITE_WARDIAN_WORKBENCH === "1",
  };
}

export const WORKBENCH_FLAGS = resolveWorkbenchFlags(import.meta.env);
