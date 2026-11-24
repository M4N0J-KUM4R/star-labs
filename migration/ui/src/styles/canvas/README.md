# Generated output

`npm run styles:canvas` writes one compiled Canvas bundle per page here
(`common.css`, `dashboard.css`, `assignments.css`, ...).

A page imports only the bundles it renders, mirroring Canvas loading one bundle
per page rather than all 139:

```ts
import '$styles/canvas/dashboard.css';
```

They are committed after generation and reviewed as a vendored artifact. Do not
hand-edit one: add the rule upstream in Canvas' SCSS, or leave it out.
