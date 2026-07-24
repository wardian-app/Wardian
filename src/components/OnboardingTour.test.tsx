import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { OnboardingTour } from "./OnboardingTour";

describe("OnboardingTour", () => {
  it("moves through the core work loops and closes when complete", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();

    render(<OnboardingTour onClose={onClose} />);

    expect(screen.getByText("Start with a reliable agent")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Back" })).toBeDisabled();
    expect(screen.getByRole("link", { name: "First-run guide" })).toHaveAttribute(
      "href",
      "https://docs.wardian.org/guide/getting-started",
    );

    await user.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("Keep the roster readable")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Next" }));
    await user.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("Turn repeatable work into workflows")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Done" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("can close without completing the tour", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();

    render(<OnboardingTour onClose={onClose} />);
    await user.click(screen.getByRole("button", { name: "Close guided tour" }));

    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
