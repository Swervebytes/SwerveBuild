import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { Chat, Project, Workspace } from "./types";

export async function loadWorkspace(): Promise<Workspace> {
  return invoke<Workspace>("get_workspace");
}

export async function pickProjectFolder(): Promise<Project | null> {
  const selected = await open({
    directory: true,
    multiple: false,
    title: "Select project folder",
  });

  if (!selected || typeof selected !== "string") {
    return null;
  }

  return invoke<Project>("add_project", { path: selected });
}

export async function createChat(projectId: string): Promise<Chat> {
  return invoke<Chat>("create_chat", { projectId });
}

export function chatsForProject(workspace: Workspace, projectId: string): Chat[] {
  return workspace.chats
    .filter((chat) => chat.project_id === projectId)
    .sort((a, b) => Number(b.updated_at) - Number(a.updated_at));
}

export function projectById(workspace: Workspace, projectId: string): Project | undefined {
  return workspace.projects.find((project) => project.id === projectId);
}