export interface ShellOption {
  id: string;
  label: string;
  executable: string;
  default_args: string[];
}

export interface ShellSettings {
  shell_id: string;
  custom_executable: string | null;
  custom_args: string | null;
  agent_session_persistence: 'fresh' | 'resume';
}
