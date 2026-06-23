export type ApprovalChoice = { value: string; label: string };

export function parseApprovalChoices(text: string): ApprovalChoice[] {
  const numbered = parseNumberedApprovalChoices(text);
  if (numbered.length > 0) return numbered;

  if (!looksLikeApprovalPrompt(text)) return [];
  return [
    { value: "y", label: "Yes" },
    { value: "n", label: "No" },
  ];
}

function parseNumberedApprovalChoices(text: string): ApprovalChoice[] {
  const choices: ApprovalChoice[] = [];
  let current: ApprovalChoice | null = null;

  text.replace(/\r\n|\r/g, "\n").split("\n").forEach((line) => {
    const match = line.match(/^\s*(?:[>*-]\s*)?(\d{1,2})[.)]\s+(.+?)\s*$/);
    if (match) {
      current = { value: match[1], label: normalizeApprovalLabel(match[2]) };
      choices.push(current);
      return;
    }

    if (current && shouldAppendApprovalContinuation(line)) {
      current.label = normalizeApprovalLabel(`${current.label} ${line.trim()}`);
    }
  });

  return choices.filter((choice) => choice.label.length > 0);
}

function normalizeApprovalLabel(label: string): string {
  return label.replace(/\s+/g, " ").trim();
}

function shouldAppendApprovalContinuation(line: string): boolean {
  const trimmed = line.trim();
  if (!trimmed) return false;
  if (/^(esc|ctrl|tab|enter|shift|navigate)\b/i.test(trimmed)) return false;
  if (/^(command|do you want|requesting permission|action required)\b/i.test(trimmed)) return false;
  return !/^[─-]{3,}$/.test(trimmed);
}

function looksLikeApprovalPrompt(text: string): boolean {
  return /\b(action required|approval|required permission|requesting permission|do you want to proceed|approve|deny)\b/i.test(text);
}
