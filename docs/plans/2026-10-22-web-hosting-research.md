# Research: Hosting for sunlit.earth

## Question

Where to host four things on the `sunlit.earth` domain, which is registered with Porkbun and stays there:

1. A static website that shows what the app looks like and does, and helps people download and install it, with basic usage metrics.
2. An update check endpoint the app calls.
3. The cloud layer the app downloads, served from our own domain instead of straight from `clouds.matteason.co.uk`.
4. Later, a job that generates our own cloud map every couple of hours and publishes it in place of the upstream one.

## What the app asks of a host today

The cloud fetcher (`crates/sunlit-core/src/assets/cloud_fetcher.rs`) polls `https://clouds.matteason.co.uk/images/{size}/clouds.jpg` once an hour with a conditional request (`ETag` and `Last-Modified` are cached beside the image), where `{size}` is `8192x4096`, `4096x2048` or `2048x1024` from the texture resolution setting. The requests carry the user agent `sunlit.earth/<version>`. The 8K JPEG is about 5 MB, 4 to 6 MB depending on cloud cover ([2026-03-20-live-clouds-research.md](2026-03-20-live-clouds-research.md)). Upstream regenerates it every 30 minutes on a GitHub Actions schedule, which queueing stretches to 60 to 90 minutes in practice.

That makes bandwidth the dominant cost, not the website. An 8K client that downloads about 8 new maps a day moves about 40 MB a day, 1.2 TB a month per 1000 clients. The hourly polls are 24 requests a day per client, almost all of them answered with 304.

## Cloudflare

Figures as published on 2026-10-22.

- **Static assets on Workers:** requests to static assets are free and unlimited and do not count toward the Worker request quota, unless a Worker script is configured to run first ([static assets billing](https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/)).
- **Workers:** the free plan allows 100,000 requests a day, reset at midnight UTC. Workers Paid is $5 a month minimum per account ([Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/)). Cron Triggers cost nothing extra and count as requests; on Workers Paid a trigger with an interval of an hour or more gets 15 minutes of CPU time ([Workers limits](https://developers.cloudflare.com/workers/platform/limits/)).
- **R2:** no egress fees, through the Workers API, the S3 API or a public bucket. Free each month on Standard storage: 10 GB-months of storage, 1 million Class A (write) and 10 million Class B (read) operations; beyond that $0.36 per million reads ([R2 pricing](https://developers.cloudflare.com/r2/pricing/)). Reads answered from Cloudflare's cache in front of a custom domain do not reach R2 ([R2 and cache](https://developers.cloudflare.com/cache/interaction-cloudflare-products/r2/)).
- **Containers:** public beta, Workers Paid only, billed only while a container runs, with a monthly allowance included (25 GiB-hours of memory, 375 vCPU-minutes) ([Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/)). A Cron Trigger can start one; Cloudflare documents that pattern ([cron container](https://developers.cloudflare.com/containers/examples/cron/), [beta announcement](https://blog.cloudflare.com/containers-are-available-in-public-beta-for-simple-global-and-programmable/)).
- **Web Analytics:** cookieless page view analytics, free; it can also be added to a site hosted elsewhere through a JavaScript snippet.
- **Workers Analytics Engine:** a Worker can write one data point per request with a few string and number fields and query counts later with SQL. Its free allowance is in the table under "Which parts can bill".

### Keeping Porkbun as the registrar

Cloudflare's free plan needs the zone on Cloudflare's nameservers: R2 custom domains need the domain as a zone in the same account, and the alternative that leaves DNS elsewhere, a partial (CNAME) setup, is for Business and Enterprise plans only ([R2 public buckets](https://developers.cloudflare.com/r2/buckets/public-buckets/), [partial setup](https://developers.cloudflare.com/dns/zone-setups/partial-setup/setup/)). Changing the nameservers at Porkbun does that and leaves the registration at Porkbun. DNS records are then edited in Cloudflare. Porkbun's URL forwarding relies on Porkbun's DNS and stops working; whether Porkbun's email forwarding keeps working with external nameservers was not checked. DNSSEC, if on, is turned off at Porkbun before the switch and turned back on with Cloudflare's DS record after it.

### Which parts can bill

The requirement is that under extreme load the site slows down or stops, and never runs up a bill. On Cloudflare's free plans that splits the products cleanly ([Workers limits](https://developers.cloudflare.com/workers/platform/limits/), [Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/), [Analytics Engine pricing](https://developers.cloudflare.com/analytics/analytics-engine/pricing/)):

| Product | Free allowance | Past the allowance |
|---|---|---|
| Static assets on Workers | Unlimited requests; 20,000 files per version, 25 MiB per file ([static asset limits](https://developers.cloudflare.com/changelog/post/2025-09-02-increased-static-asset-limits/)) | Nothing to pass |
| Workers Free | 100,000 requests a day; 10 ms CPU per request and per cron run; 5 Cron Triggers; 50 subrequests per invocation; 128 MB memory | Requests fail (Error 1027), nothing is billed |
| Analytics Engine | 100,000 data points written and 10,000 queries a day, kept three months | Operations fail |
| Web Analytics | Free | |
| R2 | 10 GB-months, 1 million writes, 10 million reads a month | Billed, with no cap |

R2 is the one product in the design above that bills past its allowance, and community reports say enabling it needs a payment method on file even for the free tier ([community thread](https://community.cloudflare.com/t/why-using-r2-free-tier-involves-giving-card-info/945179)); no official page was found saying so. An account that never enables R2 or Workers Paid uses only products that fail closed, and with no payment method on file it cannot be billed at all.

Static assets can carry the cloud maps: the 8K map is about 5 MB against the 25 MiB file limit. Each asset gets an `ETag` from its content hash, and the default `Cache-Control` is `public, max-age=0, must-revalidate`, changeable with a `_headers` file ([static asset headers](https://developers.cloudflare.com/workers/static-assets/headers/)). Pages, which serves assets the same way, documents answering a matching `If-None-Match` with 304 ([serving Pages](https://developers.cloudflare.com/pages/configuration/serving-pages/)). For Workers static assets it is not documented, but tested on 2026-10-22 against the placeholder site on `sunlit.earth`: the 151 KB header image came with `ETag`, `Cache-Control: public, max-age=0, must-revalidate` and `CF-Cache-Status: HIT`, and a request with its `ETag` in `If-None-Match` got a 304. Updating an asset means deploying a new version of the Worker, and Wrangler uploads only the files whose hash changed. No limit on how often a Worker may be deployed or how many versions it keeps was found; that was not researched further.

Cloudflare's Service-Specific Terms (updated 2025-08-11) say that outside Enterprise, video and a disproportionate share of pictures or other large files must go through paid services "such as the Developer Platform, Images, or Stream", and that Cloudflare may limit or disable CDN access otherwise, with reasonable efforts to notify first ([terms](https://www.cloudflare.com/service-specific-terms-application-services)). Whether static assets on the free Workers plan count is not spelled out. The stated consequence is restriction, not billing.

GitHub Actions can do the scheduled work for free: standard runners are free and unlimited in public repositories ([runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners), [billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)). Scheduled runs can start late under load, and scheduled workflows in a public repository are disabled after 60 days without repository activity ([trigger events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows)).

## All static

The update check needs no code: the app asks for a file that changes when a release is published, as it asks for a cloud map that changes when upstream publishes one. With every change made by a GitHub Actions deploy, every host name can serve static files only, which changes the requirements. The site and `latest.json` are a few MB, change on releases and see little traffic; 10,000 installs checking twice a day, mostly answered with 304, move well under 1 GB a month. Almost any static host fits them. The cloud maps, at about 1.2 TB a month per 1000 8K clients and a deploy every one to three hours, are what decides the host.

### GitHub Pages

A live Pages site tested on 2026-10-22 sends `ETag`, `Last-Modified` and `Cache-Control: max-age=600`, and answers a matching `If-None-Match` with 304. Deploys from a custom Actions workflow are exempt from the build limit. Pages fits the site and `latest.json`, with DNS left at Porkbun. Its 100 GB monthly soft limit holds about 80 8K cloud clients, so it does not fit the maps. Putting Cloudflare's CDN in front of Pages, as upstream does, needs the zone on Cloudflare anyway, and is the case Cloudflare's terms restrict most plainly: large images cached by the CDN from an outside origin.

### Splitting vendors by host name

Serving only `clouds.sunlit.earth` from Cloudflare while `sunlit.earth` stays on Porkbun's DNS needs a partial (CNAME) setup or a subdomain setup, both on Cloudflare's business plans only ([partial setup](https://developers.cloudflare.com/dns/zone-setups/partial-setup/setup/)). Maps on Cloudflare therefore mean the whole zone on Cloudflare, and then the site gains nothing from being elsewhere.

### Hosts for the maps

| Host | Free allowance | Past the allowance | At 1000 8K clients |
|---|---|---|---|
| Cloudflare static assets | Unlimited | Nothing to pass; restriction after notice under the terms | $0, zone on Cloudflare |
| GitHub Pages | 100 GB soft | Support email, then possibly suspension | Too small |
| Netlify free | 300 credits, about 15 GB at 20 credits per GB per third-party guides (unverified) ([credit plans](https://www.netlify.com/changelog/netlify-pricing-update-introducing-credit-based-plans/), [free plan limits](https://netli.fyi/blog/netlify-free-plan-limits-2026)) | Every site of the team pauses until the cycle resets | Too small |
| Vercel Hobby | About 100 GB, non-commercial (not verified) | Paused (not verified) | Too small |
| Bunny.net | Prepaid balance | Usage keeps being billed into a negative balance; without auto-recharge the account is disabled within four days of going negative, and storage keeps accruing while it is ([negative balance](https://bunny.net/docs/billing/negative-balance), [auto recharge](https://bunny.net/docs/billing/auto-recharge), [add funds](https://bunny.net/docs/billing/add-funds)) | About $6 to $12 a month |
| Oracle Cloud Always Free | 10 TB egress a month, on a VM we run ([free tier](https://oracle.com/at/cloud/free)) | Not found; one third-party source says egress past 10 TB is billed ([breakdown](https://selfhost.it.com/blog/oci-free-tier-breakdown)) | $0 and a server to run |
| Hetzner VPS | 20 TB included | Believed billed per TB; not verified | About €4 to €6.50 a month |

Only Cloudflare is both free and unlimited. Bunny is the only paid host with something close to a hard cap, and it overshoots.

### Counting without a Worker

With no code running per request, nothing can be counted at request time. Cloudflare's GraphQL Analytics API offers `httpRequestsAdaptiveGroups` on every plan, with at least 31 days of retention since October 2026, up to 30 days per query, and 300 queries per 5 minutes per user ([GraphQL limits](https://developers.cloudflare.com/analytics/graphql-api/limits/), [31 days on every plan](https://developers.cloudflare.com/changelog/post/2026-10-02-30-days-analytics-on-every-plan/), [Security Analytics](https://developers.cloudflare.com/waf/analytics/security-analytics/)). The data is sampled. The change that extended retention did not change which fields each plan can read, and whether the free plan can group by request path and user agent was not found. The app already sends `sunlit.earth/<version>` as its user agent for the cloud download, so adding the OS and architecture to it would make version, OS and architecture countable from those fields if they are available.

## Alternatives considered

### GitHub Pages

Free, and the deploy is a GitHub Actions workflow in the repository the site lives in. Custom domains are set up with DNS records, which can stay at Porkbun. It serves only static files, so the update check could only be a JSON file and nothing could be counted from it. It has no analytics of its own; Cloudflare Web Analytics, GoatCounter or Plausible would be added with a snippet.

It cannot serve the clouds. Pages has a soft bandwidth limit of 100 GB a month and a 1 GB site limit, and GitHub's terms say Pages "may not be appropriate for some high-bandwidth uses" ([Pages limits](https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits), [additional products terms](https://docs.github.com/en/site-policy/github-terms/github-terms-for-additional-products-and-features)). 100 GB is about 80 8K clients. Upstream itself is served from GitHub Pages behind Cloudflare.

Verdict: fine for the website alone, which would leave the clouds and the API needing a second provider anyway.

### Bunny.net

An EU company (Slovenia) with a CDN, object storage, Edge Scripting for small endpoints and Magic Containers for jobs, all pay as you go with a $1 monthly minimum. Pull zones are attached with CNAME records, so DNS can stay at Porkbun. CDN traffic is $0.01/GB in Europe and North America on the standard network, more in Asia, South America and Africa, or a flat $0.005/GB on the Volume network ([CDN pricing](https://docs.bunny.net/cdn/pricing), [pricing](https://bunny.net/pricing/), [storage pricing](https://bunny.net/docs/storage/pricing)). The storage rate per GB and the Edge Scripting price could not be confirmed.

At 1.2 TB a month per 1000 8K clients the clouds cost about $6 to $12 a month per 1000 clients, growing linearly. Verdict: the best option if the domain's DNS has to stay at Porkbun or an EU provider is wanted for every part, at a cost that grows with the user count where Cloudflare's does not.

### One Hetzner VPS

A single small server in the EU: Caddy serves the site and the cloud files, a small Rust service answers the update check, a systemd timer runs the mirror and later the generator. EU plans include 20 TB of traffic a month, about 16,000 8K clients. DNS stays at Porkbun. The smallest plans cost about €4 to €6.50 a month with an IPv4 address after Hetzner's 2026 price increases, but the sources disagree on current prices, and one reports the cheapest lines as not orderable since June 2026 ([price comparison 2026-08](https://kimmo.suominen.com/stuff/cpc-2026-08.txt), [price increase](https://findstack.com/resources/hetzner-price-increase-2026)).

What it costs that the others do not: we run, patch and monitor a server; one machine in one place is the uptime; analytics are self-hosted (Umami, GoatCounter) or read from logs. Verdict: the simplest system to reason about and the most flexible for the generator, at a fixed cost and with the operations work.

### Ruled out without a closer look

Netlify and Vercel: generous for static sites and functions, but both meter bandwidth on their free tiers at a level the clouds would pass quickly, and Vercel's free tier is for non-commercial use. Both pause rather than bill on their free tiers (see "Hosts for the maps" above). AWS, Google Cloud and Azure: object storage egress is billed per GB at rates above Bunny's, so the clouds would cost the most there.

## Other findings

- If the app's cloud URL points at our own domain, the source behind it can change without a release. A URL compiled into a release lives as long as anyone runs that release, so this is worth doing before anything else, even while the bytes still come from upstream.
- A mirror that fetches upstream once per change and serves every client from our side cuts the load on upstream from one conditional request per client per hour to one per mirror run.
- Our own cloud maps will not look like upstream's under the same shader settings: the haze removal, the floors and the gammas were tuned on upstream's infrared composite ([2026-10-10-cloud-haze-research.md](2026-10-10-cloud-haze-research.md)). A versioned path lets a later release switch to a map with different semantics while older releases keep the one they were tuned for.
- Generating our own maps from EUMETSAT data is subject to EUMETSAT's data licensing; upstream's output is CC0. That needs its own look when the generator is planned.
- A website run from Germany will most likely need an Impressum and a privacy notice, and the notice has to cover the analytics and what the update check records.
