export type QueueActionChoice = {
  value: string;
  label: string;
};

export function parseQueueActionChoices(text: string | null | undefined): QueueActionChoice[] {
  const source = text?.trim() ?? "";
  if (!source) return [];

  const numbered = parseNumberedChoices(source);
  if (numbered.length > 0) return numbered;

  return [];
}

function parseNumberedChoices(text: string): QueueActionChoice[] {
  const choices: QueueActionChoice[] = [];
  let current: QueueActionChoice | null = null;

  text.replace(/\r\n|\r/g, "\n").split("\n").forEach((line) => {
    const match = line.match(/^\s*(?:[>*-]\s*)?(\d{1,2})[.)]\s+(.+?)\s*$/);
    if (match) {
      current = { value: match[1], label: normalizeChoiceLabel(match[2]) };
      choices.push(current);
      return;
    }

    if (current && shouldAppendContinuation(line)) {
      current.label = normalizeChoiceLabel(`${current.label} ${line.trim()}`);
    }
  });

  return choices.filter((choice) => choice.label.length > 0);
}

function normalizeChoiceLabel(label: string): string {
  return label.replace(/\s+/g, " ").trim();
}

function shouldAppendContinuation(line: string): boolean {
  const trimmed = line.trim();
  if (!trimmed) return false;
  if (/^(esc|ctrl|tab|enter|shift|navigate)\b/i.test(trimmed)) return false;
  if (/^(command|do you want|requesting permission|action required)\b/i.test(trimmed)) return false;
  return !/^[─-]{3,}$/.test(trimmed);
}
