import type { Config } from 'tailwindcss'

export default {
  darkMode: 'class',
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      colors: {
        app: '#0A0A0A',
        window: '#101112',
        card: '#141517',
        row: 'rgba(255,255,255,0.035)',
        line: 'rgba(255,255,255,0.08)',
        linecard: 'rgba(255,255,255,0.06)',
        t1: '#FFFFFF',
        t2: 'rgba(255,255,255,0.58)',
        t3: 'rgba(255,255,255,0.34)',
        ok: '#2FD463',
        warn: '#E8B437',
        crit: '#E5484D',
        idle: 'rgba(255,255,255,0.28)',
        navactive: 'rgba(47,212,99,0.10)',
        info: '#4DC3FF',
        /* shadcn semantic aliases */
        background: '#0A0A0A',
        foreground: '#FFFFFF',
        primary: { DEFAULT: '#2FD463', foreground: '#0A0A0A' },
        destructive: { DEFAULT: '#E5484D', foreground: '#FFFFFF' },
        muted: 'rgba(255,255,255,0.045)',
        'muted-foreground': 'rgba(255,255,255,0.58)',
        accent: 'rgba(255,255,255,0.06)',
        'accent-foreground': '#FFFFFF',
        border: 'rgba(255,255,255,0.08)',
        input: 'rgba(255,255,255,0.08)',
        ring: '#2FD463',
      },
      fontFamily: {
        sans: ['Inter', 'Segoe UI Variable Text', 'Segoe UI', 'system-ui', 'sans-serif'],
        mono: ['JetBrains Mono', 'Cascadia Mono', 'Consolas', 'monospace'],
      },
      borderRadius: {
        lg: '10px',
        xl: '16px',
      },
    },
  },
  plugins: [require('tailwindcss-animate')],
} satisfies Config
