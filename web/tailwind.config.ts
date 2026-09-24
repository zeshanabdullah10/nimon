import type { Config } from 'tailwindcss'

/* Colours are CSS variables (src/index.css) so dark/light share one class set.
 * Text tokens t1/t2/t3 are ≥ 4.5:1 on card/window backgrounds in both themes. */
const v = (name: string) => `rgb(var(--${name}) / <alpha-value>)`

export default {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      colors: {
        app: v('app'),
        window: v('window'),
        card: v('card'),
        fg: v('fg'),
        t1: v('t1'),
        t2: v('t2'),
        t3: v('t3'),
        ok: v('ok'),
        warn: v('warn'),
        crit: v('crit'),
        info: v('info'),
        idle: v('t3'),
        onaccent: v('onaccent'),
      },
      fontFamily: {
        sans: ['"Segoe UI Variable Text"', '"Segoe UI"', 'system-ui', '-apple-system', 'Roboto', '"Helvetica Neue"', 'Arial', 'sans-serif'],
        mono: ['"Cascadia Mono"', 'Consolas', 'ui-monospace', '"SF Mono"', 'Menlo', 'monospace'],
      },
      borderRadius: { lg: '10px', xl: '14px' },
    },
  },
  plugins: [],
} satisfies Config
