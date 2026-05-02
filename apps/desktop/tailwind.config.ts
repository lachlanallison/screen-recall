import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        bg: {
          DEFAULT: "#0b0d10",
          elevated: "#12151a",
          hover: "#1a1f26",
        },
        border: {
          DEFAULT: "#232932",
          strong: "#2e3540",
        },
        text: {
          DEFAULT: "#e6e8eb",
          muted: "#8b95a1",
          faint: "#5b6573",
        },
        accent: {
          DEFAULT: "#7c9cff",
          hover: "#9bb3ff",
        },
      },
      fontFamily: {
        sans: [
          "Inter",
          "system-ui",
          "-apple-system",
          "Segoe UI",
          "sans-serif",
        ],
        mono: ["JetBrains Mono", "SFMono-Regular", "Menlo", "monospace"],
      },
      boxShadow: {
        card: "0 1px 2px rgba(0,0,0,0.4), 0 4px 12px rgba(0,0,0,0.25)",
      },
    },
  },
  plugins: [],
} satisfies Config;
