import type { IconName } from "$lib/components/ui/icons";

export type NavItem = {
  id: string;
  label: string;
  href: string;
  icon: IconName;
};

export const navItems: NavItem[] = [
  { id: "home", label: "Home", href: "/", icon: "home" },
  { id: "projects", label: "Projects", href: "/projects", icon: "folder" },
  { id: "automations", label: "Automations", href: "/automations", icon: "zap" },
  { id: "workflows", label: "Workflows", href: "/workflows", icon: "workflow" },
  { id: "memories", label: "Memories", href: "/memories", icon: "memory" },
  { id: "skills", label: "Skills", href: "/skills", icon: "skills" },
  { id: "settings", label: "Settings", href: "/settings", icon: "settings" },
];
