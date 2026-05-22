export const normalizedRemoteAgentStatus = (status: string): string =>
  status
    .trim()
    .toLowerCase()
    .replace(/\.+$/g, "")
    .replace(/\s+/g, "_");

export const isRemoteAgentOff = (status: string): boolean => {
  const normalized = normalizedRemoteAgentStatus(status);
  return normalized === "off" || normalized === "offline";
};

export const remoteStatusClassFor = (status: string): string => {
  switch (normalizedRemoteAgentStatus(status)) {
    case "idle":
      return "bg-wardian-success";
    case "processing":
    case "running":
      return "bg-wardian-processing";
    case "action_required":
    case "action_needed":
      return "bg-wardian-warning";
    case "error":
    case "failed":
      return "bg-wardian-error";
    default:
      return "bg-wardian-off";
  }
};
