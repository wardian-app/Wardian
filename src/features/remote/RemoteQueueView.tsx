import React from "react";
import { Inbox } from "lucide-react";

export const RemoteQueueView: React.FC = () => (
  <section className="border-t border-wardian-border px-3 py-4">
    <div className="mb-3 flex items-center gap-2">
      <Inbox className="h-4 w-4 text-muted-neutral" aria-hidden="true" />
      <h2 className="text-xs font-semibold uppercase text-muted-neutral">Queue</h2>
    </div>
    <div className="rounded-md border border-dashed border-wardian-border px-3 py-4 text-xs text-muted-neutral">
      No remote queue items.
    </div>
  </section>
);
