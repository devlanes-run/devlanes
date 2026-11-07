// @ts-check
// The style variables each simulated host sends the card in `hostContext.styles`, for the
// preview pages only. Values follow what each host is known to send: Claude's dump in
// anthropics/claude-ai-mcp#202, ChatGPT as sunpeak's simulator captured it (no state
// backgrounds or borders in light mode), VS Code's `--vscode-*` references resolved for
// its default Light and Dark 2026 themes, Goose's theme-tokens.ts and the ext-apps
// basic-host example. "devlanes" sends nothing, so the card uses its own palette.
(() => {
  /** @typedef {Record<string, [string | null, string | null]>} Colors light and dark per name, null when not sent */

  /** @type {Colors} */
  const claude = {
    'background-primary': ['rgba(255, 255, 255, 1)', 'rgba(48, 48, 46, 1)'],
    'background-secondary': ['rgba(245, 244, 237, 1)', 'rgba(38, 38, 36, 1)'],
    'background-tertiary': ['rgba(250, 249, 245, 1)', 'rgba(20, 20, 19, 1)'],
    'background-inverse': ['rgba(20, 20, 19, 1)', 'rgba(250, 249, 245, 1)'],
    'background-ghost': ['rgba(255, 255, 255, 0)', 'rgba(48, 48, 46, 0)'],
    'background-info': ['rgba(214, 228, 246, 1)', 'rgba(37, 62, 95, 1)'],
    'background-danger': ['rgba(247, 236, 236, 1)', 'rgba(96, 42, 40, 1)'],
    'background-success': ['rgba(233, 241, 220, 1)', 'rgba(27, 70, 20, 1)'],
    'background-warning': ['rgba(246, 238, 223, 1)', 'rgba(72, 58, 15, 1)'],
    'background-disabled': ['rgba(255, 255, 255, 0.5)', 'rgba(48, 48, 46, 0.5)'],
    'text-primary': ['rgba(20, 20, 19, 1)', 'rgba(250, 249, 245, 1)'],
    'text-secondary': ['rgba(61, 61, 58, 1)', 'rgba(194, 192, 182, 1)'],
    'text-tertiary': ['rgba(115, 114, 108, 1)', 'rgba(156, 154, 146, 1)'],
    'text-inverse': ['rgba(255, 255, 255, 1)', 'rgba(20, 20, 19, 1)'],
    'text-ghost': ['rgba(115, 114, 108, 0.5)', 'rgba(156, 154, 146, 0.5)'],
    'text-info': ['rgba(50, 102, 173, 1)', 'rgba(128, 170, 221, 1)'],
    'text-danger': ['rgba(127, 44, 40, 1)', 'rgba(238, 136, 132, 1)'],
    'text-success': ['rgba(38, 91, 25, 1)', 'rgba(122, 185, 72, 1)'],
    'text-warning': ['rgba(90, 72, 21, 1)', 'rgba(209, 160, 65, 1)'],
    'text-disabled': ['rgba(20, 20, 19, 0.5)', 'rgba(250, 249, 245, 0.5)'],
    'border-primary': ['rgba(31, 30, 29, 0.4)', 'rgba(222, 220, 209, 0.4)'],
    'border-secondary': ['rgba(31, 30, 29, 0.3)', 'rgba(222, 220, 209, 0.3)'],
    'border-tertiary': ['rgba(31, 30, 29, 0.15)', 'rgba(222, 220, 209, 0.15)'],
    'border-inverse': ['rgba(255, 255, 255, 0.3)', 'rgba(20, 20, 19, 0.15)'],
    'border-ghost': ['rgba(31, 30, 29, 0)', 'rgba(222, 220, 209, 0)'],
    'border-info': ['rgba(70, 130, 213, 1)', 'rgba(70, 130, 213, 1)'],
    'border-danger': ['rgba(167, 61, 57, 1)', 'rgba(205, 92, 88, 1)'],
    'border-success': ['rgba(67, 116, 38, 1)', 'rgba(89, 145, 48, 1)'],
    'border-warning': ['rgba(128, 92, 31, 1)', 'rgba(168, 120, 41, 1)'],
    'border-disabled': ['rgba(31, 30, 29, 0.1)', 'rgba(222, 220, 209, 0.1)'],
    'ring-primary': ['rgba(20, 20, 19, 0.7)', 'rgba(250, 249, 245, 0.7)'],
    'ring-secondary': ['rgba(61, 61, 58, 0.7)', 'rgba(194, 192, 182, 0.7)'],
    'ring-inverse': ['rgba(255, 255, 255, 0.7)', 'rgba(20, 20, 19, 0.7)'],
    'ring-info': ['rgba(50, 102, 173, 0.5)', 'rgba(128, 170, 221, 0.5)'],
    'ring-danger': ['rgba(167, 61, 57, 0.5)', 'rgba(205, 92, 88, 0.5)'],
    'ring-success': ['rgba(67, 116, 38, 0.5)', 'rgba(89, 145, 48, 0.5)'],
    'ring-warning': ['rgba(128, 92, 31, 0.5)', 'rgba(168, 120, 41, 0.5)'],
  };

  /** @type {Colors} */
  const chatgpt = {
    'background-primary': ['#fff', '#212121'],
    'background-secondary': ['#f9f9f9', '#181818'],
    'background-tertiary': ['#f3f3f3', '#131313'],
    'background-inverse': ['#181818', '#f3f3f3'],
    'background-ghost': ['color-mix(in oklab, #0d0d0d 0%, transparent)', 'color-mix(in oklab, #fff 0%, transparent)'],
    'background-info': [null, '#0285ff21'],
    'background-danger': [null, '#fa423e29'],
    'background-success': [null, '#04b84c26'],
    'background-warning': [null, '#fb6a2229'],
    'background-disabled': ['color-mix(in oklab, #0d0d0d 5%, transparent)', 'color-mix(in oklab, #fff 5%, transparent)'],
    'text-primary': ['#0d0d0d', '#fff'],
    'text-secondary': ['#5d5d5d', '#cdcdcd'],
    'text-tertiary': ['#8f8f8f', '#8f8f8f'],
    'text-inverse': ['#fff', '#0d0d0d'],
    'text-ghost': ['#0d0d0d', '#fff'],
    'text-info': ['#3a83f7', '#539af8'],
    'text-danger': ['#fa423e', '#ff6764'],
    'text-success': ['#2c6732', '#53b559'],
    'text-warning': ['#87401d', '#d25e28'],
    'text-disabled': ['#8f8f8f', '#5d5d5d'],
    'border-primary': ['color-mix(in oklab, #0d0d0d 10%, transparent)', 'color-mix(in oklab, #fff 12%, transparent)'],
    'border-secondary': ['color-mix(in oklab, #0d0d0d 5%, transparent)', 'color-mix(in oklab, #fff 6%, transparent)'],
    'border-tertiary': ['color-mix(in oklab, #0d0d0d 15%, transparent)', 'color-mix(in oklab, #fff 20%, transparent)'],
    'border-inverse': ['#fff', '#0d0d0d'],
    'border-ghost': ['color-mix(in oklab, #0d0d0d 0%, transparent)', 'color-mix(in oklab, #fff 0%, transparent)'],
    'border-info': [null, '#0285ff21'],
    'border-danger': [null, '#fa423e29'],
    'border-success': [null, '#04b84c26'],
    'border-warning': [null, '#fb6a2229'],
    'border-disabled': ['color-mix(in oklab, #0d0d0d 6%, transparent)', 'color-mix(in oklab, #fff 6%, transparent)'],
    'ring-primary': ['#2c67c5', '#3a83f7'],
    'ring-secondary': ['#2c67c5', '#3a83f7'],
    'ring-inverse': ['#2c67c5', '#3a83f7'],
    'ring-info': ['#2c67c5', '#3a83f7'],
    'ring-danger': ['#ff8583', '#ff8583'],
    'ring-success': ['#2c67c5', '#3a83f7'],
    'ring-warning': ['#2c67c5', '#3a83f7'],
  };

  /** @type {Colors} */
  const vscode = {
    'background-primary': ['#ffffff', '#121314'],
    'background-secondary': ['#fafafd', '#191a1b'],
    'background-tertiary': ['#fafafd', '#191a1b'],
    'background-inverse': ['#202020', '#bbbebf'],
    'background-ghost': ['transparent', 'transparent'],
    'background-info': ['#e6f2fa', '#1e3a47'],
    'background-danger': ['#fdeded', '#3a1d1d'],
    'background-success': ['#587c0c26', '#57ab5a4d'],
    'background-warning': ['#fdf6e3', '#352a05'],
    'background-disabled': ['#0069cc1a', '#27678260'],
    'text-primary': ['#202020', '#bfbfbf'],
    'text-secondary': ['#606060', '#8c8c8c'],
    'text-tertiary': ['#bbbbbb', '#555555'],
    'text-inverse': ['#ffffff', '#121314'],
    'text-ghost': ['#606060', '#8c8c8c'],
    'text-info': ['#0069cc', '#48a0c7'],
    'text-danger': ['#ad0707', '#f48771'],
    'text-success': ['#73c991', '#73c991'],
    'text-warning': ['#bf8803', '#cca700'],
    'text-disabled': ['#bbbbbb', '#555555'],
    'border-primary': ['#e2e2e5', '#2a2b2c'],
    'border-secondary': ['#e4e5e6', '#2a2b2c'],
    'border-tertiary': ['#f0f1f2', '#2a2b2c'],
    'border-inverse': ['#202020', '#bfbfbf'],
    'border-ghost': ['transparent', 'transparent'],
    'border-info': ['#0069cc', '#3994bc'],
    'border-danger': ['#ad0707', '#be1100'],
    'border-success': ['#73c991', '#73c991'],
    'border-warning': ['#b69500', '#b89500'],
    'border-disabled': ['#bbbbbb', '#555555'],
    'ring-primary': ['#0069cc', '#3994bcb3'],
    'ring-secondary': ['#0069cc', '#3994bcb3'],
    'ring-inverse': ['#0069cc', '#3994bcb3'],
    'ring-info': ['#0069cc', '#3994bc'],
    'ring-danger': ['#ad0707', '#be1100'],
    'ring-success': ['#73c991', '#73c991'],
    'ring-warning': ['#b69500', '#b89500'],
  };

  /** @type {Colors} */
  const goose = {
    'background-primary': ['#ffffff', '#22252a'],
    'background-secondary': ['#f4f6f7', '#3f434b'],
    'background-tertiary': ['#e3e6ea', '#474e57'],
    'background-inverse': ['#000000', '#cbd1d6'],
    'background-ghost': ['transparent', 'transparent'],
    'background-info': ['#5c98f9', '#7cacff'],
    'background-danger': ['#f94b4b', '#ff6b6b'],
    'background-success': ['#91cb80', '#a3d795'],
    'background-warning': ['#fbcd44', '#ffd966'],
    'background-disabled': ['#e3e6ea', '#474e57'],
    'text-primary': ['#3f434b', '#ffffff'],
    'text-secondary': ['#878787', '#878787'],
    'text-tertiary': ['#a7b0b9', '#606c7a'],
    'text-inverse': ['#ffffff', '#000000'],
    'text-ghost': ['#878787', '#878787'],
    'text-info': ['#5c98f9', '#7cacff'],
    'text-danger': ['#f94b4b', '#ff6b6b'],
    'text-success': ['#91cb80', '#a3d795'],
    'text-warning': ['#fbcd44', '#ffd966'],
    'text-disabled': ['#cbd1d6', '#525b68'],
    'border-primary': ['#e3e6ea', '#3f434b'],
    'border-secondary': ['#e3e6ea', '#525b68'],
    'border-tertiary': ['#cbd1d6', '#474e57'],
    'border-inverse': ['#000000', '#ffffff'],
    'border-ghost': ['transparent', 'transparent'],
    'border-info': ['#5c98f9', '#7cacff'],
    'border-danger': ['#f94b4b', '#ff6b6b'],
    'border-success': ['#91cb80', '#a3d795'],
    'border-warning': ['#fbcd44', '#ffd966'],
    'border-disabled': ['#e3e6ea', '#3f434b'],
    'ring-primary': ['#e3e6ea', '#525b68'],
    'ring-secondary': ['#cbd1d6', '#474e57'],
    'ring-inverse': ['#ffffff', '#000000'],
    'ring-info': ['#5c98f9', '#7cacff'],
    'ring-danger': ['#f94b4b', '#ff6b6b'],
    'ring-success': ['#91cb80', '#a3d795'],
    'ring-warning': ['#fbcd44', '#ffd966'],
  };

  /** @type {Colors} */
  const basic = {
    'background-primary': ['#ffffff', '#1a1a1a'],
    'background-secondary': ['#f5f5f5', '#2d2d2d'],
    'background-tertiary': ['#e5e5e5', '#404040'],
    'background-inverse': ['#1a1a1a', '#ffffff'],
    'background-ghost': ['rgba(255, 255, 255, 0)', 'rgba(26, 26, 26, 0)'],
    'background-info': ['#eff6ff', '#1e3a5f'],
    'background-danger': ['#fef2f2', '#7f1d1d'],
    'background-success': ['#f0fdf4', '#14532d'],
    'background-warning': ['#fefce8', '#713f12'],
    'background-disabled': ['rgba(255, 255, 255, 0.5)', 'rgba(26, 26, 26, 0.5)'],
    'text-primary': ['#1f2937', '#f3f4f6'],
    'text-secondary': ['#6b7280', '#9ca3af'],
    'text-tertiary': ['#9ca3af', '#6b7280'],
    'text-inverse': ['#f3f4f6', '#1f2937'],
    'text-ghost': ['rgba(107, 114, 128, 0.5)', 'rgba(156, 163, 175, 0.5)'],
    'text-info': ['#1d4ed8', '#60a5fa'],
    'text-danger': ['#b91c1c', '#f87171'],
    'text-success': ['#15803d', '#4ade80'],
    'text-warning': ['#a16207', '#fbbf24'],
    'text-disabled': ['rgba(31, 41, 55, 0.5)', 'rgba(243, 244, 246, 0.5)'],
    'border-primary': ['#e5e7eb', '#404040'],
    'border-secondary': ['#d1d5db', '#525252'],
    'border-tertiary': ['#f3f4f6', '#374151'],
    'border-inverse': ['rgba(255, 255, 255, 0.3)', 'rgba(0, 0, 0, 0.3)'],
    'border-ghost': ['rgba(229, 231, 235, 0)', 'rgba(64, 64, 64, 0)'],
    'border-info': ['#93c5fd', '#1e40af'],
    'border-danger': ['#fca5a5', '#991b1b'],
    'border-success': ['#86efac', '#166534'],
    'border-warning': ['#fde047', '#854d0e'],
    'border-disabled': ['rgba(229, 231, 235, 0.5)', 'rgba(64, 64, 64, 0.5)'],
    'ring-primary': ['#3b82f6', '#60a5fa'],
    'ring-secondary': ['#6b7280', '#9ca3af'],
    'ring-inverse': ['#ffffff', '#1f2937'],
    'ring-info': ['#2563eb', '#3b82f6'],
    'ring-danger': ['#dc2626', '#ef4444'],
    'ring-success': ['#16a34a', '#22c55e'],
    'ring-warning': ['#ca8a04', '#eab308'],
  };

  /** The weights every host sends the same way, as numbers. */
  const weights = { normal: '400', medium: '500', semibold: '600', bold: '700' };

  /**
   * The non-colour variables: font stacks, weights, the text and heading sizes and
   * line heights (xs..lg, xs..3xl), radii (xs..xl, full), border width and shadows.
   * @param {object} p
   * @param {string} p.sans
   * @param {string} p.mono
   * @param {Record<string, string>} p.weights
   * @param {string[]} p.text sizes xs, sm, md, lg
   * @param {string[]} p.textLh
   * @param {string[]} p.heading sizes xs, sm, md, lg, xl, 2xl, 3xl
   * @param {string[]} p.headingLh
   * @param {string[]} p.radius xs, sm, md, lg, xl, full
   * @param {string} p.border
   * @param {string[]} [p.shadows] hairline, sm, md, lg
   * @returns {Record<string, string>}
   */
  function rest(p) {
    /** @type {Record<string, string>} */
    const out = { '--font-sans': p.sans, '--font-mono': p.mono, '--border-width-regular': p.border };
    for (const [name, value] of Object.entries(p.weights)) out[`--font-weight-${name}`] = value;
    ['xs', 'sm', 'md', 'lg'].forEach((size, i) => {
      out[`--font-text-${size}-size`] = p.text[i] || '';
      out[`--font-text-${size}-line-height`] = p.textLh[i] || '';
    });
    ['xs', 'sm', 'md', 'lg', 'xl', '2xl', '3xl'].forEach((size, i) => {
      out[`--font-heading-${size}-size`] = p.heading[i] || '';
      out[`--font-heading-${size}-line-height`] = p.headingLh[i] || '';
    });
    ['xs', 'sm', 'md', 'lg', 'xl', 'full'].forEach((size, i) => {
      out[`--border-radius-${size}`] = p.radius[i] || '';
    });
    ['hairline', 'sm', 'md', 'lg'].forEach((size, i) => {
      if (p.shadows?.[i]) out[`--shadow-${size}`] = p.shadows[i];
    });
    return out;
  }

  const tailwindShadows = [
    '0 1px 2px 0 rgba(0, 0, 0, 0.05)',
    '0 1px 3px 0 rgba(0, 0, 0, 0.1), 0 1px 2px -1px rgba(0, 0, 0, 0.1)',
    '0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -2px rgba(0, 0, 0, 0.1)',
    '0 10px 15px -3px rgba(0, 0, 0, 0.1), 0 4px 6px -4px rgba(0, 0, 0, 0.1)',
  ];

  /** Anthropic Sans from Claude's font CDN, as Claude's `styles.css.fonts` names it. */
  const claudeFonts = [
    ['Regular', 400],
    ['Medium', 500],
    ['Semibold', 600],
    ['Bold', 700],
  ]
    .map(
      ([name, weight]) =>
        `@font-face { font-family: "Anthropic Sans"; font-weight: ${weight}; font-style: normal; font-display: swap; src: url("https://assets.claude.ai/Fonts/AnthropicSans-Text-${name}-Static.otf") format("opentype"); }`,
    )
    .join('\n');

  /**
   * One simulated host.
   * @typedef {object} Host
   * @property {string} label
   * @property {boolean} border whether it draws a frame for `prefersBorder: true`
   * @property {[string, string]} page the chat background, light and dark
   * @property {Colors | null} colors
   * @property {boolean} lightDark whether it sends `light-dark()` pairs instead of one theme's values
   * @property {boolean} themeOnly whether its change notifications carry only `{theme}`
   * @property {Record<string, string>} rest
   * @property {string} fonts its `styles.css.fonts`
   * @property {boolean} fullscreen whether it lists fullscreen in `availableDisplayModes`
   */

  /** @type {Record<string, Host>} */
  const HOSTS = {
    devlanes: {
      label: 'devlanes (no variables)',
      border: true,
      page: ['#e9e7e1', '#22211e'],
      colors: null,
      lightDark: false,
      themeOnly: false,
      rest: {},
      fonts: '',
      fullscreen: true,
    },
    claude: {
      label: 'Claude',
      border: true,
      page: ['#faf9f5', '#262624'],
      colors: claude,
      lightDark: true,
      themeOnly: true,
      rest: rest({
        sans: 'Anthropic Sans, sans-serif',
        mono: 'ui-monospace, monospace',
        weights,
        text: ['12px', '14px', '16px', '20px'],
        textLh: ['1.4', '1.4', '1.4', '1.25'],
        heading: ['12px', '14px', '16px', '20px', '24px', '28px', '36px'],
        headingLh: ['1.4', '1.4', '1.4', '1.25', '1.25', '1.1', '1'],
        radius: ['4px', '6px', '8px', '10px', '12px', '9999px'],
        border: '0.5px',
        shadows: tailwindShadows,
      }),
      fonts: claudeFonts,
      fullscreen: true,
    },
    chatgpt: {
      label: 'ChatGPT',
      border: true,
      page: ['#ffffff', '#212121'],
      colors: chatgpt,
      lightDark: false,
      themeOnly: false,
      rest: rest({
        sans: "'ui-sans-serif', '-apple-system', 'system-ui', 'Segoe UI', 'Helvetica', 'Apple Color Emoji', 'Arial', 'sans-serif', 'Segoe UI Emoji', 'Segoe UI Symbol'",
        mono: "'ui-monospace', 'SFMono-Regular', 'SF Mono', 'Menlo', 'Consolas', 'Liberation Mono', 'monospace'",
        weights,
        text: ['0.75rem', '0.875rem', '1rem', '1.125rem'],
        textLh: ['1.125rem', '1.25rem', '1.5rem', '1.8125rem'],
        heading: ['1rem', '1.125rem', '1.25rem', '1.5rem', '2rem', '2.25rem', '3rem'],
        headingLh: ['1.5rem', '1.625rem', '1.625rem', '1.75rem', '2.375rem', '2.625rem', '3rem'],
        radius: ['0.25rem', '0.375rem', '0.5rem', '0.625rem', '12px', '9999px'],
        border: '1px',
      }),
      fonts: '',
      fullscreen: true,
    },
    vscode: {
      label: 'VS Code',
      border: false,
      page: ['#fafafd', '#191a1b'],
      colors: vscode,
      lightDark: false,
      themeOnly: false,
      rest: rest({
        sans: "'Segoe WPC', 'Segoe UI', sans-serif",
        mono: "Consolas, 'Courier New', monospace",
        weights: { normal: 'normal', medium: '500', semibold: '600', bold: 'bold' },
        text: ['10px', '11px', '13px', '14px'],
        textLh: ['1.5', '1.5', '1.5', '1.5'],
        heading: ['16px', '18px', '20px', '24px', '32px', '40px', '48px'],
        headingLh: ['1.25', '1.25', '1.25', '1.25', '1.25', '1.25', '1.25'],
        radius: ['2px', '3px', '4px', '6px', '8px', '9999px'],
        border: '1px',
      }),
      fonts: '',
      fullscreen: false,
    },
    goose: {
      label: 'Goose',
      border: true,
      page: ['#f4f6f7', '#22252a'],
      colors: goose,
      lightDark: true,
      themeOnly: false,
      rest: rest({
        // Goose sends Cash Sans from its own CDN, which the card's policy does not allow,
        // so the preview leaves its font rules out and the stack falls back.
        sans: "'Cash Sans', sans-serif",
        mono: 'monospace',
        weights,
        text: ['0.75rem', '0.875rem', '1rem', '1.125rem'],
        textLh: ['1rem', '1.25rem', '1.5rem', '1.75rem'],
        heading: ['1rem', '1.125rem', '1.25rem', '1.5rem', '1.875rem', '2.25rem', '3rem'],
        headingLh: ['1.5rem', '1.75rem', '1.75rem', '2rem', '2.25rem', '2.5rem', '3.5rem'],
        radius: ['2px', '4px', '8px', '12px', '16px', '9999px'],
        border: '1px',
        shadows: tailwindShadows,
      }),
      fonts: '',
      fullscreen: true,
    },
    'basic-host': {
      label: 'ext-apps basic-host',
      border: true,
      page: ['#f5f5f5', '#2d2d2d'],
      colors: basic,
      lightDark: true,
      themeOnly: true,
      rest: rest({
        sans: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif",
        mono: "ui-monospace, 'SF Mono', Monaco, 'Cascadia Code', monospace",
        weights,
        text: ['0.75rem', '0.875rem', '1rem', '1.125rem'],
        textLh: ['1.4', '1.4', '1.5', '1.5'],
        heading: ['0.75rem', '0.875rem', '1rem', '1.25rem', '1.5rem', '1.875rem', '2.25rem'],
        headingLh: ['1.4', '1.4', '1.4', '1.3', '1.25', '1.2', '1.1'],
        radius: ['2px', '4px', '6px', '8px', '12px', '9999px'],
        border: '1px',
        shadows: tailwindShadows,
      }),
      fonts: '',
      fullscreen: true,
    },
  };

  /**
   * The `styles.variables` a host sends in `theme`: `light-dark()` pairs for hosts that
   * send them, else that theme's values, leaving out what the host leaves out.
   * @param {string} name
   * @param {'light' | 'dark'} theme
   * @returns {Record<string, string>}
   */
  function variables(name, theme) {
    const host = HOSTS[name];
    if (!host?.colors) return {};
    /** @type {Record<string, string>} */
    const out = {};
    for (const [key, [light, dark]] of Object.entries(host.colors)) {
      const value = host.lightDark ? `light-dark(${light}, ${dark})` : theme === 'dark' ? dark : light;
      if (value) out[`--color-${key}`] = value;
    }
    return { ...out, ...host.rest };
  }

  /** @type {any} */ (window).HOSTS = { hosts: HOSTS, variables };
})();
