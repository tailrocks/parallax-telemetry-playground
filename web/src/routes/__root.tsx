/// <reference types="vite/client" />
import { HeadContent, Scripts, createRootRoute } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { CartProvider } from "../cart";
import { CartStorageNotice, SiteHeader } from "../components";
import "../styles.css";

export const Route = createRootRoute({
  head: () => ({
    meta: [
      { charSet: "utf-8" },
      { name: "viewport", content: "width=device-width, initial-scale=1" },
      { title: "Parallax Commerce Lab" },
    ],
  }),
  shellComponent: RootDocument,
});

function RootDocument({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <head>
        <HeadContent />
      </head>
      <body>
        <CartProvider>
          <SiteHeader />
          <CartStorageNotice />
          {children}
          <footer className="footer">
            <span>
              Parallax Commerce Lab · Catalog → pricing → checkout → fulfillment
            </span>
          </footer>
        </CartProvider>
        <Scripts />
      </body>
    </html>
  );
}
