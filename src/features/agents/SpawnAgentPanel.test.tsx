import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { SpawnAgentPanel } from "./SpawnAgentPanel";

describe("SpawnAgentPanel", () => {
  it("lists OpenCode as a provider option", () => {
    render(
      <SpawnAgentPanel
        agentClasses={[{ name: "Generalist", description: "", is_default: true }]}
        onSpawned={() => {}}
      />,
    );

    const providerSelect = screen.getByTestId("spawn-provider");
    expect(screen.getByRole("option", { name: "OpenCode" })).toBeInTheDocument();
    expect(providerSelect).toHaveTextContent("OpenCode");
  });
});
