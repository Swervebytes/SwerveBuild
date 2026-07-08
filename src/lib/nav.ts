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
  { id: "memories", label: "Memories", href: "/memories", icon: "memory" },
  { id: "skills", label: "Skills", href: "/skills", icon: "skills" },
  { id: "terminal", label: "Terminal", href: "/terminal", icon: "terminal" },
  { id: "settings", label: "Settings", href: "/settings", icon: "settings" },
];
