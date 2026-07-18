import {
  forwardRef,
  type ButtonHTMLAttributes,
} from "react";
import { Ellipsis } from "lucide-react";

import "./CompactOverflowButton.css";

export type CompactOverflowButtonProps = Omit<
  ButtonHTMLAttributes<HTMLButtonElement>,
  "children" | "type"
>;

/** Shared compact ellipsis action used by Workbench surface toolbars. */
export const CompactOverflowButton = forwardRef<
  HTMLButtonElement,
  CompactOverflowButtonProps
>(function CompactOverflowButton({ className, ...props }, ref) {
  return (
    <button
      {...props}
      ref={ref}
      type="button"
      className={["wardian-compact-overflow-trigger", className]
        .filter(Boolean)
        .join(" ")}
      data-hit-size="22"
    >
      <Ellipsis size={14} strokeWidth={1.75} aria-hidden="true" />
    </button>
  );
});
