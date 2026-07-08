import { defineConfig } from 'vitepress'

// https://vitepress.dev/reference/site-config
export default defineConfig({
  title: 'CKB Controller',
  description: 'Session-based game accounts & session keys on Nervos CKB',

  // GitHub Project Pages are served under /<repo>/. If you rename the repo or
  // use a custom domain / user-org page, change this to '/' accordingly.
  base: '/ckb-controller/',

  cleanUrls: true,
  lastUpdated: true,

  // Serve the section READMEs as their directory index, so /guide/ and
  // /internals/ work while the files keep their GitHub-friendly names.
  rewrites: {
    'guide/README.md': 'guide/index.md',
    'internals/README.md': 'internals/index.md',
  },

  themeConfig: {
    nav: [
      { text: 'Guide', link: '/guide/' },
      { text: 'Internals', link: '/internals/' },
    ],

    sidebar: {
      '/guide/': [
        {
          text: 'Game developer guide',
          items: [
            { text: 'Overview', link: '/guide/' },
            { text: 'Quickstart', link: '/guide/quickstart' },
            { text: 'The session model', link: '/guide/sessions' },
            { text: 'Trust & safety', link: '/guide/trust' },
            { text: "Writing your game's rules", link: '/guide/your-game' },
            { text: 'Configuration', link: '/guide/configuration' },
            { text: 'Going live on testnet', link: '/guide/going-live' },
          ],
        },
      ],
      '/internals/': [
        {
          text: 'Maintainer documentation',
          items: [
            { text: 'Overview', link: '/internals/' },
            { text: 'Architecture', link: '/internals/architecture' },
            { text: 'Wire formats', link: '/internals/wire-formats' },
            { text: 'Invariants', link: '/internals/invariants' },
            { text: 'Deployments', link: '/internals/deployments' },
            { text: 'Test map', link: '/internals/test-map' },
          ],
        },
      ],
    },

    search: { provider: 'local' },

    socialLinks: [
      { icon: 'github', link: 'https://github.com/Kagwep/ckb-controller' },
    ],
  },
})
