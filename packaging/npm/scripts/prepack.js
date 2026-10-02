'use strict';

// Copy the license files from the repository root so they ship in the npm tarball.
const fs = require('fs');
const path = require('path');

const pkg = path.join(__dirname, '..');
const repo = path.join(pkg, '..', '..');
for (const f of ['LICENSE-MIT', 'LICENSE-APACHE']) {
  fs.copyFileSync(path.join(repo, f), path.join(pkg, f));
}
