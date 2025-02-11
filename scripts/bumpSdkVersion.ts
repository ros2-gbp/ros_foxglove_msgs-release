import { spawnSync } from "child_process";
import { appendFile, readFile, writeFile } from "fs/promises";
import { glob } from "glob";
import path from "path";
import semver from "semver";

const versionRegex = /^version\s*=\s*"([^"]*)"/m;

async function main() {
  const newVersionV = process.argv[2];
  if (newVersionV?.startsWith("v") !== true) {
    console.log("Usage: bumpSdkVersion.ts <version>");
    console.log("<version> must start with 'v'");
    process.exit(1);
  }

  // Remove the 'v' prefix
  const newVersion = newVersionV.slice(1);
  if (!semver.valid(newVersion)) {
    console.log("Usage: bumpSdkVersion.ts <version>");
    console.log(`"${newVersionV}" is not a valid semver version`);
    process.exit(1);
  }

  // Find all Cargo.toml files in the workspace
  const workspaceRoot = path.resolve(__dirname, "..");
  const cargoFiles = await glob("**/Cargo.toml", {
    ignore: ["**/target/**", "**/node_modules/**"],
    cwd: workspaceRoot,
    absolute: true,
  });

  let success = true;
  let prevVersion: string | undefined;

  for (const cargoFile of cargoFiles) {
    console.log(`Checking ${cargoFile}...`);
    const content = await readFile(cargoFile, "utf8");

    if (!versionRegex.test(content)) {
      console.log(`  ℹ️ Skipped, does not contain version field`);
      continue;
    }

    prevVersion = versionRegex.exec(content)?.[1] ?? "";

    // check that newVersion is greater than prevVersion
    if (semver.compare(newVersion, prevVersion) <= 0) {
      console.error(
        `  ❌ New version ${newVersion} must be greater than previous version ${prevVersion}`,
      );
      success = false;
      continue;
    }

    const updatedContent = content.replace(versionRegex, `version = "${newVersion}"`);
    if (content === updatedContent) {
      console.error(`  ❌ Version could not be updated from "${prevVersion}" to "${newVersion}"`);
      success = false;
    } else {
      await writeFile(cargoFile, updatedContent);
      console.log(`  ✅ Updated version in ${cargoFile} to ${newVersion}`);
    }
  }

  if (!success || !prevVersion) {
    console.error("\n❌ Some versions could not be updated");
    process.exit(1);
  }

  // run cargo tree --workspace
  // we don't need the output, only check the exit code
  console.log("\nValidating Cargo.toml...");
  const { status, stderr } = spawnSync("cargo", ["tree", "--workspace"], {
    cwd: workspaceRoot,
  });

  if (status !== 0) {
    console.error(stderr.toString());
    console.error("\n❌ Failed to validate Cargo.toml");
    process.exit(status);
  }

  console.log("\n✅ Success!");

  // github action outputs
  const githubOutput = process.env.GITHUB_OUTPUT;
  if (githubOutput) {
    await appendFile(
      path.resolve(githubOutput),
      `prev-version=${prevVersion}\nnew-version=${newVersion}\n`,
    );
  }
}

main().catch((err: unknown) => {
  console.error(err);
  process.exit(1);
});
