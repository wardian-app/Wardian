import { createRef } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { CompactOverflowButton } from "./CompactOverflowButton";

describe("CompactOverflowButton", () => {
  it("provides one shared compact overflow geometry", () => {
    const ref = createRef<HTMLButtonElement>();
    const onClick = vi.fn();

    render(
      <CompactOverflowButton
        ref={ref}
        className="consumer-trigger"
        aria-label="More actions"
        aria-expanded="false"
        onClick={onClick}
      />,
    );

    const trigger = screen.getByRole("button", { name: "More actions" });
    expect(trigger).toBe(ref.current);
    expect(trigger).toHaveClass("wardian-compact-overflow-trigger", "consumer-trigger");
    expect(trigger).toHaveAttribute("data-hit-size", "22");
    expect(trigger).toHaveAttribute("type", "button");
    expect(trigger.querySelector("svg.lucide-ellipsis")).toHaveAttribute("width", "14");

    fireEvent.click(trigger);
    expect(onClick).toHaveBeenCalledOnce();
  });
});
