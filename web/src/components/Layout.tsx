import type { ReactNode } from "react";
import AppLayout from "@cloudscape-design/components/app-layout";
import TopNavigation from "@cloudscape-design/components/top-navigation";

interface LayoutProps {
  children: ReactNode;
}

export default function Layout({ children }: LayoutProps) {
  return (
    <>
      <TopNavigation
        identity={{
          href: "/",
          title: "USG TFTP File Manager",
        }}
        i18nStrings={{
          overflowMenuTriggerText: "More",
          overflowMenuTitleText: "All",
        }}
      />
      <AppLayout
        content={children}
        toolsHide
        navigationHide
        contentType="table"
      />
    </>
  );
}
