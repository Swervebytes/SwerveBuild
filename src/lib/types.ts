export type Project = {
  id: string;
  name: string;
  path: string;
  created_at: string;
  last_opened_at: string;
};

export type ChatMessage = {
  id: string;
  role: string;
  content: string;
  images: string[];
  created_at: string;
};

export type Chat = {
  id: string;
  project_id: string;
  title: string;
  created_at: string;
  updated_at: string;
  messages: ChatMessage[];
  grok_session_id?: string | null;
};

export type Workspace = {
  projects: Project[];
  chats: Chat[];
};

export type ProviderKind = "acp" | "http";

export type Provider = {
  id: string;
  label: string;
  kind: ProviderKind;
  command: string | null;
  args: string[];
  env: [string, string][];
  accent: string;
  model: string | null;
  base_url: string | null;
  builtin: boolean;
};

export type ProviderView = Provider & {
  available: boolean;
  active: boolean;
};

export type ProviderStatus = {
  installed: boolean;
  version: string | null;
  path: string | null;
  available: boolean;
  kind: ProviderKind;
};

export type PermissionRequest = {
  chatId: string;
  requestId: number;
  params: {
    sessionId?: string;
    toolCall?: { title?: string; kind?: string; toolCallId?: string };
    options?: Array<{ optionId: string; name: string; kind: string }>;
  };
};