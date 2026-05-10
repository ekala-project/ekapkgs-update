# GitHub Pages Setup Guide

This document explains how to configure GitHub Pages for the ekapkgs-update documentation.

## Prerequisites

The repository already has:
- ✅ Documentation in `book/` directory (mdBook format)
- ✅ GitHub Actions workflow at `.github/workflows/deploy-docs.yml`
- ✅ Proper configuration in `book/book.toml`

## Initial Setup (One-Time)

After merging this PR to master, you need to enable GitHub Pages in the repository settings:

### Step 1: Enable GitHub Pages

1. Go to your repository on GitHub
2. Click **Settings** (top navigation)
3. Click **Pages** (left sidebar under "Code and automation")
4. Under **Source**, select **GitHub Actions**

   ![GitHub Pages Source Selection](https://docs.github.com/assets/cb-47267/mw-1440/images/help/pages/publishing-source-drop-down.webp)

5. Click **Save** (if required)

### Step 2: Verify Deployment

After the first workflow runs:

1. Go to **Actions** tab in your repository
2. Look for the "Deploy Documentation" workflow
3. Ensure it completes successfully
4. Visit: https://ekala-project.github.io/ekapkgs-update/

The documentation should now be live!

## How It Works

### Automatic Deployment

The workflow (`.github/workflows/deploy-docs.yml`) automatically deploys documentation when:

1. **Changes are pushed to `master`** that affect:
   - Files in `book/` directory
   - The workflow file itself (`.github/workflows/deploy-docs.yml`)

2. **Manual trigger** via GitHub Actions UI

### Workflow Steps

```yaml
1. Checkout repository code
2. Install mdBook (latest version)
3. Build documentation (mdbook build)
4. Add .nojekyll file (prevents Jekyll processing)
5. Upload built documentation as artifact
6. Deploy artifact to GitHub Pages
```

### Permissions

The workflow requires these permissions (already configured):

```yaml
permissions:
  contents: read      # Read repository contents
  pages: write        # Deploy to GitHub Pages
  id-token: write     # OIDC token for deployment
```

### Concurrency Control

Only one deployment runs at a time:

```yaml
concurrency:
  group: "pages"
  cancel-in-progress: false
```

This prevents race conditions if multiple PRs are merged quickly.

## Manually Triggering Deployment

You can manually deploy documentation without pushing changes:

1. Go to **Actions** tab
2. Select **Deploy Documentation** workflow
3. Click **Run workflow** button
4. Select `master` branch
5. Click **Run workflow**

## Troubleshooting

### Deployment Fails

**Check workflow logs:**
1. Go to **Actions** tab
2. Click on the failed workflow run
3. Click on the failed job to see detailed logs

**Common issues:**

#### Build fails
```
Error: mdbook build failed
```
**Solution:** Test locally first:
```bash
cd book
mdbook build
```

#### Permission denied
```
Error: Resource not accessible by integration
```
**Solution:** Verify Pages is set to "GitHub Actions" source in Settings > Pages

#### 404 after deployment
**Solution:** Ensure `site-url = "/ekapkgs-update/"` is set in `book/book.toml`

### Documentation Not Updating

1. **Check if workflow ran:**
   - Go to Actions tab
   - Verify "Deploy Documentation" workflow completed successfully

2. **Check if files changed:**
   - Workflow only runs when files in `book/` change
   - Or when workflow file itself changes

3. **Clear browser cache:**
   - GitHub Pages uses CDN caching
   - Hard refresh: Ctrl+Shift+R (Windows/Linux) or Cmd+Shift+R (Mac)

4. **Check GitHub Pages URL:**
   - Should be: https://ekala-project.github.io/ekapkgs-update/
   - Not: https://ekala-project.github.io/ekapkgs-update/book/

### Custom Domain (Optional)

To use a custom domain (e.g., docs.ekapkgs.org):

1. **Add CNAME file:**
   ```bash
   echo "docs.ekapkgs.org" > book/src/CNAME
   ```

2. **Update workflow:**
   ```yaml
   - name: Copy CNAME
     run: cp book/src/CNAME book/book/CNAME
   ```

3. **Configure DNS:**
   - Add CNAME record pointing to: `ekala-project.github.io`

4. **Update GitHub Pages settings:**
   - Settings > Pages > Custom domain
   - Enter: `docs.ekapkgs.org`
   - Enable "Enforce HTTPS"

5. **Update `book/book.toml`:**
   ```toml
   [output.html]
   site-url = "/"
   ```

## Local Development

### Preview Documentation Locally

```bash
cd book
mdbook serve
```

Opens at: http://localhost:3000

### Build Documentation Locally

```bash
cd book
mdbook build
```

Output in: `book/book/`

### Watch for Changes

```bash
cd book
mdbook watch
```

Rebuilds automatically when files change.

## Documentation Structure

```
book/
├── book.toml           # mdBook configuration
├── src/                # Documentation source (Markdown)
│   ├── SUMMARY.md      # Table of contents
│   ├── introduction.md
│   ├── installation.md
│   ├── quick-start.md
│   ├── cli-reference.md
│   ├── configuration.md
│   ├── passthru-attributes.md
│   ├── passthru-attributes/
│   ├── usage/
│   ├── advanced/
│   └── contributing/
└── book/               # Build output (generated)
    ├── index.html
    ├── *.html
    └── .nojekyll       # Disables Jekyll processing
```

## Updating Documentation

### Adding a New Page

1. Create new `.md` file in `book/src/`:
   ```bash
   touch book/src/my-new-page.md
   ```

2. Add to table of contents in `book/src/SUMMARY.md`:
   ```markdown
   # Summary

   - [My New Page](./my-new-page.md)
   ```

3. Write content using Markdown

4. Preview locally:
   ```bash
   cd book
   mdbook serve
   ```

5. Commit and push to trigger deployment:
   ```bash
   git add book/src/my-new-page.md book/src/SUMMARY.md
   git commit -m "docs: Add new page about X"
   git push
   ```

### Editing Existing Pages

1. Edit `.md` file in `book/src/`
2. Preview locally with `mdbook serve`
3. Commit and push changes
4. Deployment happens automatically

## Security

### Permissions

The workflow uses minimal required permissions:
- `contents: read` - Only reads repository
- `pages: write` - Only writes to Pages
- `id-token: write` - Only for GitHub OIDC

### Secrets

No secrets are required for public documentation deployment.

For private repositories, GitHub token is automatically provided.

## Maintenance

### Updating mdBook Version

The workflow uses the latest mdBook version automatically:

```yaml
- uses: peaceiris/actions-mdbook@v2
  with:
    mdbook-version: 'latest'
```

To pin to a specific version:

```yaml
mdbook-version: '0.4.40'
```

### Monitoring Deployments

1. **GitHub Actions tab** - View all deployments
2. **Environments** - View deployment history
   - Settings > Environments > github-pages

### Rollback

To rollback to a previous version:

1. Go to Actions tab
2. Find successful previous deployment
3. Click "Re-run all jobs"

Or revert the commit:

```bash
git revert <commit-hash>
git push
```

## Support

For issues with:
- **Documentation content**: Open issue in ekapkgs-update repository
- **GitHub Pages**: Check [GitHub Pages documentation](https://docs.github.com/en/pages)
- **mdBook**: Check [mdBook documentation](https://rust-lang.github.io/mdBook/)

## Resources

- [GitHub Pages Documentation](https://docs.github.com/en/pages)
- [GitHub Actions for Pages](https://github.com/actions/deploy-pages)
- [mdBook Guide](https://rust-lang.github.io/mdBook/)
- [mdBook GitHub Action](https://github.com/peaceiris/actions-mdbook)
