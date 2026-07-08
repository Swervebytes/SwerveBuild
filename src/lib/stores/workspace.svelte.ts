// Shared workspace state (Svelte 5 runes) so the Sidebar rail, Projects page,
// and Chat page all read one reactive source. Route chat/project mutations
// through refresh() afterward to keep the rail live.

import { invoke } from "@tauri-apps/api/core";
import type { Chat, Project, Workspace } from "$lib/types";

let workspace = $state<Workspace>({ projects: [], chats: [] });
let activeSessions = $state<string[]>([]);
let loaded = $state(false);

export const workspaceStore = {
  get projects() {
    return workspace.projects;
  },
  get chats() {
    return workspace.chats;
  },
  get activeSessions() {
    return activeSessions;
  },
  get loaded() {
    return loaded;
  },
  isActive(chatId: string) {
    return activeSessions.includes(chatId);
  },
  async refresh(): Promise<Workspace> {
    try {
      workspace = await invoke<Workspace>("get_workspace");
    } catch {
      // Backend unavailable — degrade to an empty workspace rather than throwing.
      workspace = { projects: [], chats: [] };
    }
    try {
      activeSessions = await invoke<string[]>("list_active_chat_sessions");
    } catch {
      activeSessions = [];
    }
    loaded = true;
    return workspace;
  },
  async refreshSessions() {
    try {
      activeSessions = await invoke<string[]>("list_active_chat_sessions");
    } catch {
      activeSessions = [];
    }
  },
  recent(limit = 7): Chat[] {
    return [...workspace.chats]
      .sort((a, b) => Number(b.updated_at) - Number(a.updated_at))
      .slice(0, limit);
  },
  chatsForProject(projectId: string): Chat[] {
    return workspace.chats
      .filter((chat) => chat.project_id === projectId)
      .sort((a, b) => Number(b.updated_at) - Number(a.updated_at));
  },
  projectById(id: string): Project | undefined {
    return workspace.projects.find((project) => project.id === id);
  },
};
