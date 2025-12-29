/** @type {import('tailwindcss').Config} */
export default {
    content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
    theme: {
        extend: {
            colors: {
                // Primary brand colors (Deep Space Navy)
                brand: {
                    primary: "#0a0f2e", // Deepest navy for backgrounds
                    secondary: "#151b40", // Lighter navy for surface contrast
                    primary: "#18181b", // Zinc 900 (Main Background - Dark Grey)
                    secondary: "#09090b", // Zinc 950 (Black - Components)
                    tertiary: "#27272a", // Zinc 800 (Borders/Accents)
                },
                accent: {
                    DEFAULT: "#f4f4f5", // Zinc 100 (White-ish - Active States)
                    hover: "#ffffff", // Pure White
                    foreground: "#000000", // Black text on accent
                },
                surface: {
                    DEFAULT: "#09090b", // Black
                    hover: "#18181b", // Slightly lighter black
                    border: "#27272a", // Zinc 800
                },
                text: {
                    primary: "#f4f4f5", // Zinc 100
                    secondary: "#a1a1aa", // Zinc 400
                    tertiary: "#52525b", // Zinc 600
                },
                status: {
                    success: "#a1a1aa", // Zinc 400 (Muted success)
                    warning: "#d4d4d8", // Zinc 300
                    error: "#71717a", // Zinc 500 (Muted error)
                }
            },
            fontFamily: {
                sans: ["Outfit", "Inter", "sans-serif"],
                mono: ["JetBrains Mono", "monospace"],
            },
            backgroundImage: {
                "gradient-radial": "radial-gradient(var(--tw-gradient-stops))",
            },
            boxShadow: {
                "subtle": "0 1px 2px 0 rgb(0 0 0 / 0.05)",
                "elevation": "0 4px 6px -1px rgb(0 0 0 / 0.1), 0 2px 4px -2px rgb(0 0 0 / 0.1)",
            },
            animation: {
                "fade-in": "fadeIn 0.2s ease-out",
                "slide-up": "slideUp 0.3s ease-out",
            },
            keyframes: {
                fadeIn: {
                    "0%": { opacity: "0" },
                    "100%": { opacity: "1" },
                },
                slideUp: {
                    "0%": { transform: "translateY(10px)", opacity: "0" },
                    "100%": { transform: "translateY(0)", opacity: "1" },
                },
            },
        },
    },
    plugins: [],
};
