# Contributing

## Generating schemas

Schemas are defined in [internal/schemas.ts](internal/schemas.ts). After modifying this file, you will need to regenerate the language-specific definitions.

```sh
corepack enable  # ensure you have corepack enabled
yarn install     # install dependencies
yarn update-generated-files
```

Remember to publish new versions of all libraries!

## Release instructions

Releases are published via GitHub Actions.

### Rust & Python

All SDK languages are versioned and released together.

1. Manually trigger the "Draft Release" workflow in GitHub Actions, specifying the new version number.
2. Check the release notes, and hit publish on the new release.
3. Ensure the post-release and tag workflows complete successfully.

### TypeScript

1. Create and merge a PR bumping the version number in `package.json` file(s).
2. Manually create a new Release in the GitHub UI. Ensure the tag uses the form `[language]/[package-name]/vX.Y.Z`.
3. GitHub Actions will take care of the rest.

### ROS

For first-time setup, follow the guides for [installing bloom](http://ros-infrastructure.github.io/bloom/) and [authenticating with GitHub](https://wiki.ros.org/bloom/Tutorials/GithubManualAuthorization).

Permissions to push to [foxglove/ros_foxglove_msgs-release](https://github.com/foxglove/ros_foxglove_msgs-release) (for ROS 1) and [ros2-gbp/ros_foxglove_msgs-release](https://github.com/ros2-gbp/ros_foxglove_msgs-release) (for ROS 2) are required. The latter are managed [via Terraform](https://github.com/ros2-gbp/ros2-gbp-github-org/blob/latest/foxglove_msgs.tf).

The following is a modified version of [bloom release instructions](https://wiki.ros.org/bloom/Tutorials/ReleaseCatkinPackage) (because catkin_generate_changelog and catkin_prepare_release can't handle our custom tag format of `ros-vX.Y.Z`).

1. Manually update `package.xml` and `CHANGELOG.rst` with new version info
2. Manually create a tag named `ros-vX.Y.Z` for the new version
3. Push the newly created commit and tag
4. Run `bloom-release foxglove_msgs --ros-distro humble`, for each distro you want to publish the release to. Follow the prompts, and the script will automatically make a PR to the [ros/rosdistro](https://github.com/ros/rosdistro) repo.

Packages will be available via apt after the [next sync](https://discourse.ros.org/c/release/16). View package build status prior to the sync at:
[noetic](http://repositories.ros.org/status_page/ros_noetic_default.html?q=foxglove),
[foxy](http://repo.ros2.org/status_page/ros_foxy_default.html?q=foxglove),
[galactic](http://repo.ros2.org/status_page/ros_galactic_default.html?q=foxglove),
[humble](http://repo.ros2.org/status_page/ros_humble_default.html?q=foxglove),
[iron](http://repo.ros2.org/status_page/ros_iron_default.html?q=foxglove),
[jazzy](http://repo.ros2.org/status_page/ros_jazzy_default.html?q=foxglove),
[rolling](http://repo.ros2.org/status_page/ros_rolling_default.html?q=foxglove)
