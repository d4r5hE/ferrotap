# Ferrotap project site

Static site served by GitHub Pages (plain HTML/CSS, no build step).

- `index.html` — landing page
- `style.css` — styles (dark + light)
- `.nojekyll` — serve files as-is (skip Jekyll)

## Enabling Pages

Once the repository is **public**, enable Pages for free:

1. Repo → **Settings → Pages**
2. **Build and deployment → Source: Deploy from a branch**
3. Branch: **`main`**, folder: **`/docs`** → **Save**

The site then publishes at `https://d4r5he.github.io/ferrotap/`
(GitHub lowercases the owner name in the URL).

> Pages from a *private* repo requires a paid GitHub plan; on a public repo it is free.

## Note on certificates

The site intentionally does **not** host a downloadable root CA. Ferrotap generates a unique
CA locally per install and its private key never leaves the user's machine; a shared, publicly
downloadable CA would be a serious security hole. The HTTPS section only documents how a user
trusts *their own* generated certificate.
