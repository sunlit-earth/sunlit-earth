# Plan: Hosting sunlit.earth on Cloudflare

## Summary

Everything on the `sunlit.earth` domain is static files, served by Cloudflare from Workers that have static assets and no script, and updated by GitHub Actions workflows that deploy them. The domain's nameservers point at Cloudflare and the registration stays at Porkbun ([2026-10-22-web-hosting-research.md](2026-10-22-web-hosting-research.md)).

The governing requirement is that the running cost is zero and stays zero: a hobby project with no SLA would rather stop serving under extreme load than run up a bill. The design meets it structurally rather than by monitoring. No request runs code, static asset requests are free and unlimited, and neither the Cloudflare account nor the GitHub organization has a payment method on file, so no usage can turn into a charge.

| Host name | Serves | Changes when | Deployed by |
|---|---|---|---|
| `sunlit.earth`, `www.sunlit.earth` | The website | The site changes, a release is published | `sunlit-earth/website` |
| `api.sunlit.earth` | `v1/latest.json`, the update check | A release is published | `sunlit-earth/website` |
| `clouds.sunlit.earth` | `v1/{w}x{h}/clouds.jpg`, the cloud maps | Upstream publishes a new map, every one to three hours | `sunlit-earth/clouds` |

It comes in four phases, each useful on its own:

1. DNS and the website.
2. The cloud mirror, and the app pointed at it.
3. The update check, in `latest.json` and in the app, and counting the checks.
4. Our own cloud maps.

## Status

As of 2026-11-08, phase 1 is done except for the release dispatch (open item 1), and phases 2 to 4 have not started: `clouds.sunlit.earth` and `api.sunlit.earth` do not resolve, the `clouds` and `metrics` repositories do not exist, and the app still polls `clouds.matteason.co.uk` with the user agent `sunlit.earth/<version>`. Step 2 has no recorded result.

- **Account:** the Cloudflare account has no payment method and R2 and Workers Paid are off. The GitHub organization has no payment method or an Actions spending limit of zero.
- **DNS:** the zone is on Cloudflare's nameservers and signed, with its DS record in the `.earth` zone. SSL/TLS is Full (strict), and `www.sunlit.earth` and `http://sunlit.earth` answer with a 301 to `https://sunlit.earth/`. The domain receives no mail and says so with a null MX record and `v=spf1 -all`. A `google-site-verification` TXT record is there for Google Search Console.
- **Website:** deployed from `sunlit-earth/website`, the placeholder on 2026-10-22 and the Zola site on 2026-10-25. How it differs from phase 1 as planned is under Departures.
- **App:** since db60eaf the About window links to `https://sunlit.earth` beside the source code, and `cargo xtask manifests` gives it as the homepage of the Scoop manifest and of the Homebrew cask and formula.

## Stakes Classification

Medium. Nothing here touches the renderer or the engine. What makes it more than low is that phase 2 and phase 3 put URLs into released binaries, which cannot be taken back: whatever paths ship have to be served for as long as anyone runs those releases.

## Key Design Decisions

### 1. Cloudflare, with DNS moved and the registration kept

Cloudflare is the one host found that serves static files without a bandwidth limit and without charge, which covers the cloud maps, the one cost that grows with the user count. Every other host either has an allowance the maps would pass at around a hundred users or charges per GB (research, "All static"). Hosting only `clouds.sunlit.earth` there is not possible on the free plan, because moving a subdomain alone needs a partial or subdomain setup, which Cloudflare sells only on its business plans. So the whole zone moves, and with it there is no reason to host the site anywhere else.

### 2. Static assets only, no product that bills, no payment method

Every host name is a Worker with static assets and no script (`wrangler.jsonc` with `assets.directory` and no `main`). Requests to static assets are unlimited and never invoke a Worker, so not even the free plan's 100,000 Worker requests a day apply. The account enables nothing billed by usage, R2 and Workers Paid included, and has no payment method. The GitHub organization has none either: standard runners are free in public repositories, and the one private repository's workflow fits in the free minutes. Anything later that needs a paid product is a deliberate change of this decision, not a step in a phase.

Under extreme load everything keeps serving. Cloudflare's terms let it restrict a free account that serves a disproportionate share of large files, after reasonable efforts to notify; that would stop the maps, which is the outcome this decision accepts over a bill.

### 3. Every change is a deploy from GitHub Actions

Nothing on the domain is written at runtime. A new release redeploys the site and `latest.json`; a new cloud map redeploys the clouds Worker. Wrangler uploads only the files whose content hash changed, and a deploy switches all of a Worker's files at once, so a client never sees a half-updated set.

### 4. One Worker per host name

The three hosts change at different rates, so each is its own Worker with its own custom domain. One Worker on several custom domains would serve every file under every host name. The clouds Worker lives in its own repository so its schedule, its frequent deploys and its keepalive commits stay out of the website's history.

### 5. Versioned paths

The maps are served at `https://clouds.sunlit.earth/v1/{w}x{h}/clouds.jpg`. `v1` means "upstream's infrared composite", the map the app's haze removal, floors and gammas were tuned on. A generated map with different semantics goes under `v2`, and the release that retunes the shader for it switches to `v2`, while older releases keep reading `v1`. If our own map turns out to be a drop-in replacement, it is written to `v1` and no release is needed. `latest.json` sits under `v1/` for the same reason: a later format is a new path, and older releases keep reading theirs.

### 6. The update check is a file, and the app decides

`latest.json` lists the newest stable release and the newest prerelease with every asset. The app picks the entry for its channel and the asset for its OS and architecture, and compares versions itself. A file cannot answer per client, and does not need to.

### 7. Counting from Cloudflare's request analytics, without identifying anyone

No code runs per request, so nothing can be counted at request time. Instead the app's user agent names its version, OS and architecture, `sunlit.earth/<version> (<os>; <arch>)`, and Cloudflare's GraphQL Analytics API, which keeps at least 31 days of sampled request data on the free plan, is queried once a day for yesterday's requests to `latest.json` grouped by user agent. No IP address and no install ID is collected by us. With the app checking at startup and once a day, daily checks approximate daily active installs; the counts are sampled, so they are estimates. Whether the free plan exposes the path and user agent fields of that dataset is not verified, and step 2 of the steps below checks it before anything depends on it. Without them the counting is GitHub's download counts and the site's request counts in the dashboard (departure 7), and nothing else changes.

## Repositories

| Repository | Visibility | Holds | Runs |
|---|---|---|---|
| `sunlit-earth/website` | Public | The Zola site, `latest.json`'s generator, the site and API Workers' configuration | On push to `main`, and on a `repository_dispatch` from a published release |
| `sunlit-earth/clouds` | Public | The clouds Worker's configuration, the mirror workflow, later the generator | Every 30 minutes |
| `sunlit-earth/metrics` | Private | The daily counts | Once a day |

The two public repositories deploy with `cloudflare/wrangler-action` and an API token scoped to editing Workers on this account. The metrics workflow uses a separate token scoped to reading analytics. Both are repository secrets.

## Phase 1: DNS and the website

### Account

1. Create a Cloudflare account on the free plan. Do not add a payment method, and do not enable R2 or Workers Paid.
2. Check that the GitHub organization has no payment method, or an Actions spending limit of zero.

### DNS

1. Add the `sunlit.earth` zone.
2. Compare the records Cloudflare imports with Porkbun's DNS and add what is missing, MX records included if the domain receives mail.
3. If DNSSEC is on at Porkbun, turn it off and wait for the DS record's TTL.
4. At Porkbun, replace the nameservers with the two Cloudflare assigns. Wait until Cloudflare reports the zone active.
5. Turn DNSSEC on in Cloudflare and enter its DS record at Porkbun.
6. Set SSL/TLS to Full (strict) and turn on Always Use HTTPS.

### The website

- **Generator:** Zola, a single binary with no Node toolchain, which fits a Rust project. The site is a handful of pages; nothing in it needs a framework.
- **Hosting:** the `site` Worker on the custom domains `sunlit.earth` and `www.sunlit.earth`. `www` redirects to the apex with a redirect rule.
- **Deploy:** a workflow on push to `main` builds with Zola and deploys. Pull requests build without deploying.
- **Download buttons:** the release asset names carry the version, so the site needs the current release at build time. The deploy workflow reads it with `gh release list` from `sunlit-earth/sunlit-earth`, and `package-managers.yml` gains a step that sends a `repository_dispatch` to `sunlit-earth/website` when a release is published, prereleases included, with the GitHub App token it already creates (the App needs access to the website repository). The page offers the archive for the visitor's OS first and lists the others.

Pages, with the content taken from README and the docs:

| Page | Content |
|---|---|
| Home | A large rendered globe, the features with images, the download buttons |
| Install | Scoop, Homebrew, the archives per OS, the macOS Gatekeeper step, the Linux wallpaper setters |
| Gallery | Renders in each surface mode and of the sky, made with `sunlit-earth render` |
| Privacy | What the analytics record, what the update check and the cloud download send |
| Impressum | The provider identification German law asks for; the user writes it |

The images come from `cargo app -- render` and are committed to the website repository. Making them part of each release is left for later.

### Metrics

Turn on Web Analytics for the zone in the Cloudflare dashboard. For a site proxied by Cloudflare this injects its beacon automatically, so the site carries no snippet.

### Notes from the first deploy

The first `wrangler deploy` of a new Worker can fail after the upload with `This Worker does not exist on your account. [code: 10007]`: wrangler 4.148.0 reads the Worker back through `/accounts/:id/workers/workers/:name` to reconcile `workers_dev` and `preview_urls`, and right after creation that endpoint did not know the Worker yet. Re-running the job deployed it. Expect the same on the first deploy of the API and clouds Workers.

### Done when

`https://sunlit.earth` serves the site with a valid certificate, `www` and `http` redirect to it, a push to `main` of the website repository deploys it, a published release redeploys it with the new download links, Web Analytics shows page views, and the Cloudflare account still has no payment method.

## Phase 2: The cloud mirror

### Clouds Worker

- The `clouds` Worker on the custom domain `clouds.sunlit.earth`.
- Assets at `v1/8192x4096/clouds.jpg`, `v1/4096x2048/clouds.jpg` and `v1/2048x1024/clouds.jpg`, the three variants `cloud_variant` asks for, plus `v1/upstream.json` holding upstream's `ETag` and `Last-Modified` for each.
- The default headers stay: `Cache-Control: public, max-age=0, must-revalidate` and an `ETag` from the content hash, which is what the fetcher's conditional requests need.

### Mirror workflow

A workflow in `sunlit-earth/clouds` on a `*/30 * * * *` schedule and `workflow_dispatch`. Each run:

1. Read `https://clouds.sunlit.earth/v1/upstream.json`, the state of the last deploy. The live site is the state store, so the workflow needs no cache and makes no commit per run.
2. For each variant, fetch upstream's image with `If-None-Match` and the user agent `sunlit.earth-mirror`.
3. If every variant answered 304, stop.
4. Otherwise check each new body is a JPEG of plausible size (the first bytes are `FF D8`, the length is above a floor), take the unchanged variants from `clouds.sunlit.earth`, write `upstream.json`, and deploy. A body that fails the check is logged and the variant keeps its current map.

GitHub disables a public repository's scheduled workflows after 60 days without repository activity, and deploys are not activity. The workflow therefore commits a one-line `keepalive` file when the newest commit is more than 45 days old.

Upstream regenerates its maps every 30 minutes and its runs start 60 to 90 minutes apart in practice. With our own runs also starting late under load, a new upstream map reaches clients within one to two hours, which the hourly poll already allows for.

### The app

- `CLOUD_URL_TEMPLATE` in `cloud_fetcher.rs` becomes `https://clouds.sunlit.earth/v1/{size}/clouds.jpg`. The variant, the poll interval, the conditional requests and `SUNLIT_EARTH_CLOUD_URL` stay as they are.
- The user agent in `cloud_source.rs` becomes `sunlit.earth/<version> (<os>; <arch>)`, the format decision 7 counts by, so the same release that moves the clouds also starts sending it.
- The first poll after the update sends upstream's cached `ETag` to the new host, gets a 200 and downloads once. That is accepted rather than handled.
- No fallback to upstream. One URL is easier to reason about, the mirror keeps serving the last map when upstream is down, and the environment override is the escape hatch.
- `assets/ATTRIBUTION.md` keeps crediting Matt Eason and EUMETSAT; the About window's text says the maps are mirrored at `clouds.sunlit.earth`.
- README's environment variable table names the default URL; that line changes with the user's permission.

Releases before this one keep polling upstream directly. Telling Matt Eason about the mirror is a courtesy worth doing, since it should cut upstream's traffic.

### Done when

The three maps update within two hours of upstream changing, a conditional request to `clouds.sunlit.earth` with the current `ETag` gets a 304, a release build shows clouds with only `clouds.sunlit.earth` contacted, and the engine and soak tests pass unchanged.

## Phase 3: The update check

This phase has an app side large enough for its own plan. What follows is the hosting half, the file format between the two, and the counting.

### `latest.json`

Served by the `api` Worker on the custom domain `api.sunlit.earth` at `v1/latest.json`, generated by the website's deploy workflow and deployed in the same run as the site. The workflow already reads the releases for the download links with `scripts/fetch-releases.sh`, which keeps stable releases only (departure 3). This file needs the newest prerelease as well, so the script has to keep it, and from then on the release dispatch (step 5) has to fire for prereleases too.

```json
{
  "stable": {
    "version": "0.5.0",
    "published": "2026-11-02T10:00:00Z",
    "notes": "https://github.com/sunlit-earth/sunlit-earth/releases/tag/v0.5.0",
    "assets": {
      "windows-x86_64": "https://github.com/sunlit-earth/sunlit-earth/releases/download/v0.5.0/<asset>",
      "linux-x86_64": "...",
      "linux-aarch64": "...",
      "macos-x86_64": "...",
      "macos-aarch64": "..."
    }
  },
  "prerelease": { "same fields": "for the newest prerelease, or null when none is newer than stable" }
}
```

A client on a stable version looks at `stable`. A client on a prerelease takes the newer of the two, so a beta tester moves on to the next beta or to the stable release that follows it. The file carries the default headers, so a daily check is usually a 304. Any failure means "no answer" to the app, which tries again at the next scheduled check and says nothing to the user.

### Counting

A workflow in `sunlit-earth/metrics` runs once a day, queries the GraphQL Analytics API's `httpRequestsAdaptiveGroups` for the previous UTC day's requests to `api.sunlit.earth/v1/latest.json` grouped by user agent, parses version, OS and architecture out of it, and appends the counts to a CSV in the repository. Requests to the cloud maps are counted the same way, which also covers releases that never check for updates. The repository is private so the numbers are not published by default; a run takes about a minute, inside the free minutes for private repositories. This part is built only if step 2 confirms the fields are available.

### App side, for its own plan

- A check at startup and every 24 hours, off the UI thread, with the user agent from phase 2.
- A setting to turn it off. Whether it is on by default is the user's decision, and so is where that is stated, since the site has no privacy page (departure 2).
- How a newer version is shown: in the About window and the tray menu, without downloading anything.
- Installs from Scoop and Homebrew are told to update through their package manager, which the app can tell from its install path.

### Done when

`latest.json` is redeployed by every published release with the right entry updated, a conditional request for it gets a 304, and, if counting is built, a day's CSV row matches the number of checks made by a test client within the sampling error.

## Phase 4: Our own cloud maps

Planned on its own once there is a method, with a research document on the data source, its licensing (EUMETSAT's data policy applies to their imagery, unlike upstream's CC0 output) and the processing. On the hosting side:

- The generator runs as a scheduled workflow in `sunlit-earth/clouds` on a standard runner, every one to three hours, and deploys through the same path as the mirror.
- Whether the output goes to `v1` or `v2` follows decision 5.
- The mirror stops writing whatever path the generator takes over.
- If the generator outgrows a standard runner, the options (larger runners, Cloudflare Containers, a VPS) all cost money and need decision 2 revisited first.

## Costs

Zero, apart from the domain registration at Porkbun.

| Limit | Applies to | Reached at | Then |
|---|---|---|---|
| Static asset requests | All three hosts | No limit | Cloudflare may restrict the account under its terms, after notice |
| 25 MiB per file, 20,000 files per Worker version | All three hosts | The 8K map is about 5 MB | |
| GitHub Actions in a public repository | Website, clouds | No limit | Scheduled runs start later under GitHub's load |
| GitHub Actions minutes in a private repository | Metrics | About 30 of the free monthly minutes used | Counting stops until the next month |
| GraphQL Analytics API | Metrics | 300 queries per 5 minutes; one a day is used | |

## Steps

1. Account guardrails and DNS (phase 1). Done.
2. Check that the GraphQL Analytics API on the free plan can group requests by path and user agent. (The other check planned here, that static assets answer a conditional request with 304, passed on 2026-10-22 against the placeholder site; see the research document.)
3. The clouds repository, the clouds Worker and the mirror workflow.
4. The website repository, the Zola site, the site and API Workers, `latest.json` and Web Analytics. The repository, the site and its Worker are done; Web Analytics was dropped (departure 7); the API Worker and `latest.json` belong to phase 3.
5. The `repository_dispatch` step in `package-managers.yml`. Open item 1.
6. The app change to the cloud URL and the user agent, with `docs/architecture.md` and `docs/roadmap.md` updated, in the next release.
7. The metrics repository and workflow, if step 2 found the fields.
8. The update check in the app, under its own plan.
9. The generator, under its own plan.

## Risks

- **Cloudflare's terms on large files.** The terms ask for paid services "such as the Developer Platform" for a disproportionate share of large files outside Enterprise, and it is not spelled out whether free static assets count. The consequence they state is restriction after notice, not a bill. If it happens, the maps move to another host with a DNS change and no release.
- **Deploy frequency.** The mirror deploys the clouds Worker 8 to 16 times a day, and no limit on deploys or kept versions was found. If one exists, the schedule slows down.
- **GitHub Actions scheduling.** Runs start late under load and are disabled after 60 days of inactivity, which the keepalive commit covers.
- **Counting.** It depends on fields of the free plan's analytics that are not verified, and on sampled data. Losing it loses the install estimate and nothing else.
- **Lock-in.** Every URL the app knows is on our domain, so moving any part to another host is a DNS change and a deploy, not a release.
- **Upstream ends.** The mirror keeps serving the last map, and the clouds stop updating until phase 4 or another source replaces it.

## Alternatives Considered

Details and figures in the research document.

- **R2 for the maps, written by a Cron Trigger Worker.** The conventional home for files that change, with free egress. It bills past its free allowance with no cap and needs a payment method, which decision 2 rules out.
- **A Worker answering the update check**, writing each check to Analytics Engine. It counts exactly, but fails at 100,000 requests a day, and a static file does the same job for the app with no limit.
- **GitHub Pages for the site and `latest.json`.** Free, DNS could stay at Porkbun, and it answers conditional requests with 304 (tested). Its 100 GB monthly soft limit rules it out for the maps, and once the maps need the zone on Cloudflare it adds nothing there.
- **Keeping DNS at Porkbun**, with the site on GitHub Pages and the maps on Bunny.net. Two vendors and about $6 to $12 a month per 1000 8K clients, with a prepaid balance as a soft cap that still overshoots by up to four days of usage.
- **Deferring the cloud host**, with the site on GitHub Pages and the app polling upstream until our own maps exist. Free and the least work, but every release shipped meanwhile depends on upstream's URL for as long as it runs.
- **One Hetzner VPS** would run everything for about €4 to €6.50 a month with 20 TB of traffic included, with DNS left at Porkbun, at the price of running a server.
- **Oracle Cloud Always Free** includes 10 TB of egress a month on a VM we would run; whether usage past it is blocked or billed was not found.
- **Netlify and Vercel** free tiers pause when exhausted, but at roughly 15 GB and 100 GB they hold too few cloud clients. AWS, Google Cloud and Azure bill egress per GB above Bunny's rates.

## Departures

All in phase 1. The website repository's README describes the site as built.

1. **One page instead of five.** The site is a single page: a header with the download button, a carousel of screenshots, the features, a download section with a panel per OS, and "What's new". The download panel replaces the Install page: the archive for each architecture, the Scoop and Homebrew commands with copy buttons, the system requirements, and a "First start" note per OS that covers SmartScreen on Windows and Gatekeeper on macOS. It does not list the Linux wallpaper setters; the Linux note names the desktops the app was tested on. The carousel replaces the Gallery page, with the images and their captions in `content/_index.md` and the files committed under `static/images/`, beside one screenshot of the app per OS. `site.js` opens the panel for the visitor's OS and, where the browser reports it, picks the architecture.
2. **No Privacy page and no Impressum.** Left out by the user's decision. The research's note that a site run from Germany will most likely need both stands as written there.
3. **Stable releases only, read from the REST API.** `scripts/fetch-releases.sh` reads `repos/sunlit-earth/sunlit-earth/releases` through `gh api` rather than `gh release list`, keeps published stable releases, and writes them to `data/releases.json` for the templates. It fails the build when the newest release lacks one of the seven archives the download section expects (`windows-x86_64`, `linux-x86_64`, `linux-aarch64`, and `macos-<arch>-app` and `macos-<arch>-terminal` for both architectures). Since the site never shows a prerelease, the release dispatch only needs to fire for stable releases until phase 3 (see "`latest.json`").
4. **"What's new", written by hand.** Not in the plan. The section shows the newest three stable releases and folds the older ones away, each with its date and a link to its release notes, and with a few highlights from `changes.toml` when the release has an entry there.
5. **The Worker and its headers.** The Worker is named `sunlit-earth-site`, not `site`; it has `workers_dev` and `preview_urls` off and serves the site's own page for a 404. A `_headers` file adds `Strict-Transport-Security`, `X-Content-Type-Options`, a `Referrer-Policy` and a `Content-Security-Policy` that allows only the site's own origin plus the hash of the one inline script, and gives `processed_images/`, `site.css` and `site.js` a year of `immutable` caching. Zola names the processed images by content hash and the page links the stylesheet and the script with a hash in the query string, so a change still reaches visitors. The HTML keeps the default headers.
6. **The deploy workflow.** It also runs on `workflow_dispatch`, and a pull request builds the site and runs `wrangler deploy --dry-run` rather than stopping after the build. Zola 0.23.6 is pinned by version and SHA-256, Wrangler at 4.148.0. The deploy token has two permissions, Account Workers Scripts Edit and Zone `sunlit.earth` Workers Routes Edit, and the account ID is a second secret.
7. **No Web Analytics; the site's traffic is read from the zone's request analytics.** Web Analytics counts page views with a JavaScript beacon in the page, and the user decided against any script of that kind, as against tracking cookies. The site's visits are read from the zone's HTTP traffic analytics in the Cloudflare dashboard instead, which Cloudflare records at its edge from the requests it serves, so the page carries no script for it. The served HTML has no beacon (checked on 2026-11-08), and the `Content-Security-Policy`, whose `script-src` allows only the site's own origin and the hash of its inline script, would refuse one if it were injected.

## Open items

1. **The release dispatch is not sent.** `deploy.yml` in `sunlit-earth/website` runs on a `repository_dispatch` of type `release-published`, and that repository's README says the app repository sends one, but `package-managers.yml` has no such step yet. Until it does, the download links and "What's new" follow a release only after a push to the website repository or a manual `workflow_dispatch`. The 0.5.0 release predates the Zola site, so no release has been missed so far. The `sunlit-earth-packaging` App needs access to `sunlit-earth/website` for the token it creates to send the dispatch.
