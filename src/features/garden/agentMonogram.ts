/** Pure identity mark shared by the lazy canvas and DOM Identity core. */
export function agentMonogram(label: string): string {
  return label.trim().split(/[\s_-]+/).slice(0, 2).map((part) => Array.from(part)[0] ?? "").join("").toUpperCase() || "?";
}
