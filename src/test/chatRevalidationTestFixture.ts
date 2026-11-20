import type { AgentChatPage } from "../types";
import producerFixture from "../features/chat/__fixtures__/chat-row-revalidation-producer-consumer-order-r3.json";
import emptyOwnedFixture from "../features/chat/__fixtures__/chat-empty-owned-producer-r3.json";

interface ProducerFixturePage {
  label: string;
  dto: AgentChatPage;
}

interface EmptyOwnedProducerCapture {
  label: string;
  request: {
    cursor: string | null;
    revision: string | null;
    detail_ref: string | null;
    revalidate_ids: string[] | null;
  };
  dto: AgentChatPage;
}

const producerPages = producerFixture.pages as ProducerFixturePage[];
const emptyOwnedCaptures = emptyOwnedFixture.pages as EmptyOwnedProducerCapture[];

export function actualProducerPage(label: string): AgentChatPage {
  const page = producerPages.find((candidate) => candidate.label === label);
  if (!page) throw new Error("The captured Chat producer page is missing.");
  return page.dto;
}

/** Selects only an exact captured acknowledgment; it never constructs a reply DTO. */
export function actualProducerBatch(prefix: string, requestedIds: string[]): AgentChatPage {
  const requested = new Set(requestedIds);
  const match = producerPages.find((candidate) => {
    if (!candidate.label.startsWith(prefix)) return false;
    const acknowledged = candidate.dto.revalidated_ids;
    return Array.isArray(acknowledged) && acknowledged.length === requestedIds.length
      && new Set(acknowledged).size === requested.size
      && acknowledged.every((id) => requested.has(id));
  });
  if (!match) throw new Error("The request did not match a captured Chat producer batch.");
  return match.dto;
}

export function actualEmptyOwnedCapture(label: string): EmptyOwnedProducerCapture {
  const capture = emptyOwnedCaptures.find((candidate) => candidate.label === label);
  if (!capture) throw new Error("The captured empty-owned Chat producer page is missing.");
  return capture;
}
