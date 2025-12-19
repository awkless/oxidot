<!--
SPDX-FileCopyrightText: 2025 Jason Pena <jasonpena@awkless.com>
SPDX-License-Identifier: MIT
-->

# Oxidot

Experimental dotfile organization and management tool.

## Description

Oxidot provides the user with ways to manage and organize their dotfile
configurations through __clusters__. A cluster is a bare-alias repository
whose file content can be deployed to a work tree alias. Thus, the user can
employ Git commands to directly manage configuration files without needing to
move, copy, or symlink them. The user can utilize multiple clusters such that
each cluster is housed within a cluster store.

### Bare-Alias Repositories

All clusters in Oxidot are considered _bare-alias_ repositories. Although
bare repositories lack a working tree by definition, Git allows users to
force a working tree by designating a directory as an alias for a working
tree using the “–work-tree” argument. This functionality enables us to
define a bare repository where the Git directory and the alias working tree
are kept separate. This unique feature allows us to treat an entire
directory as a Git repository without needing to initialize it as one.

This technique does not really have a standard name despite being a common
method to manage dotfile configurations through Git. So we call it the
bare-alias technique. Hence, the term bare-alias repository!

### Cluster Definition

A cluster definition is a special tracked file that specifies configuration
settings that are needed to determine how Oxidot should treat a given
cluster, e.g., give basic description of the cluster, specify the work tree
alias to use, etc.  The cluster definition can also be used to list other
clusters as dependencies of the current cluster. These dependencies will be
deployed along side their parent cluster.

All clusters must contain a valid definition file at the top-level named
"cluster.toml". If this file cannot be found, then the cluster is considered
to be invalid, i.e., not a true cluster. Thus, all clusters must be
bare-alias and contain a cluster definition file to be considered a valid
cluster.

### Deployment

Oxidot allows the user to selectively deploy the contents of a cluster to
its target work tree alias via sparse checkout. Thus, the user must supply
a listing of sparsity rules that match the file content they want deployed
to a cluster's work tree alias. By default, no component of the cluster is
deployed, unless the user specifies a default set of deployment rules to
use.

> __NOTE__: Oxidot typically uses the terms _sparsity rules_ and
> _deployment rules_ interchangeably.

## Installation

Make sure you have the following pieces of software already installed _before_
attempting to install Oxidot itself:

- openssl [>= 0.10]
- [Rust][rust-lang] [>= 2021 Edition]
- [Git][git-scm] [>= 2.30.0]

Make sure that the Cargo binary path is loaded into your `$PATH` variable.

### Cargo via crates.io

Through Cargo simply type the following into your terminal:

```
# cargo install ocd --locked
```

### Cargo locally

Clone the project, and type the following at the top-level of the project
repository:

```
# crago install --path .
```

## Usage

Firstly, lets assume that we want to define a configuration for bash. Lets
initialize a new cluster in the cluster store named "bash".

```
# oxidot init bash
```

This new cluster will be given an initial commit housing a basic cluster
definition that we can expand upon later. By default Oxidot uses our home
directory as a work tree alias. This can be changed in the cluster definition,
but for now this works for us given that bash only interprets user-level
configurations at our home directory.

Say we already have `.bash_profile` and `.bashrc` to commit into the "bash"
cluster. We do so by targeting the "bash" cluster with specific Git commands:

```
# oxidot bash add ~/.bash_profile ~/.bashrc
# oxidot bash commit -m "feat: initial bash configuration"
```

Now assume we created a remote repository to push our changes to. Lets specify
this remote to the "bash" cluster.

```
# oxidot bash remote add origin https://github.com/example/bash-cluster.git
```

Lets update the "bash" cluster's definition file to always deploy the new
files that we staged and committed. First, we need to deploy it:

```
# oxidot deploy bash "cluster.toml"
```

Now update the cluster definition accordingly:

```
[settings]
description = "Bash configuration"
work_tree_alias = "$HOME"

# Always deploy these files after cloning this cluster.
include = [".bash_profile", ".bashrc"]

[settings.remote]
url = "https://github.com/example/bash-cluster.git
```

Do not forget to stage and commit these new changes to "bash":

```
# oxidot bash add ~/cluster.toml
# oxidot bash commit -m "chore: flesh out cluster definition more"
```

Moving on, lets now assume that we have a special PS1 prompt for bash in a
separate repository at <https://github.com/example/bash-ps1.git>. This
repository comes with this cluster definition we can use:

```
[settings]
description = "Bash PS1 configuration"
work_tree_alias = "$HOME/.local/share"
include = ["ps1.sh"]

[settings.remote]
url = "https://github.com/example/bash-ps1.git
```

Thus, we can add this repository as a new cluster to our cluster store. Lets
call it "bash\_ps1". We want "bash\_ps1" to always be deployed with the "bash"
cluster as a dependency. Lets update the "bash" cluster's definition file again
to do this:

```
[settings]
description = "Bash configuration"
work_tree_alias = "$HOME"

# Always deploy these files after cloning this cluster.
include = [".bash_profile", ".bashrc"]

[settings.remote]
url = "https://github.com/example/bash-cluster.git

[[dependency]]
name = "bash_ps1"
remote = { url = "https://github.com/example/bash-ps1.git" }
```

Now any time we clone the "bash" cluster, Oxidot will also clone the "bash\_ps1"
cluster as a dependency. Given that the "bash" cluster already exists, Oxidot
will automatically resolve dependencies for it the next time we use Oxidot
itself. Remember to stage and commit the new changes to the "bash" cluster's
definition file like we did before.

> __NOTE__: Oxidot always validates the structure of the cluster store each
> time it is called. This cluster store validation process includes dependency
> resolution.

Finally, lets push our changes and reload the bash shell to enjoy our new
configuration that we defined through the "bash" cluster. We also should
undeploy the cluster definition file for "bash" to avoid cluttering our home
directory:

```
# oxidot undeploy bash "cluster.toml"
# oxidot bash push -u origin main
# exec bash
```

We now have a fully configured bash shell through the "bash" cluster! This
new cluster that we defined can also be used across multiple machines like so:

```
# oxidot clone bash https://github.com/example/bash-cluster.git
```

Use the `--help` flag for more info about Oxidot's command set. Enjoy!

> __TODO__: Include reference to wiki for more usage information at some point!

## Contributing

The Oxidot coding project is open to contribution. We accpet:

- pull requests
- feature requests
- bug reports
- bug fixes

See the [contribution guidelines][contrib-guide] for more information about
contributing to the project.

## License

The Oxidot project uses the MIT license to distribute its source code and
documentation. It also utilizes the CC0-1.0 license for files that are too
small or generic to be copyrighted, thereby placing them in the public domain.

The project uses the [REUSE 3.3 specification][reuse-3.3]. This makes it easier
to determine who owns the copyright and licensing of any given file in the
codebase. The [Developer Certificate of Origin version 1.1][linux-dco] is also
used. It ensures that contributions have the right to be merged and can be
distributed under the project's main license.

## Acknowledgements

- Arch Linux Wiki page about [dotfiles][archwiki-dotfiles], which provided a
  great introduction about using Git to manage dotfiles using the bare-alias
  technique.
- Richard Hartmann's [vcsh][vcsh-git] and [myrepos][mr-git] tools, which
  generally provided the overall look and feel of Oxidot's command set.

[rust-lang]: https://www.rust-lang.org/tools/install
[git-scm]: https://git-scm.com/downloads
[archwiki-dotfiles]: https://wiki.archlinux.org/title/Dotfiles#Tracking_dotfiles_directly_with_Git
[vcsh-git]: https://github.com/RichiH/vcsh
[mr-git]: https://github.com/RichiH/myrepos
[contrib-guide]: ./CONTRIBUTING.md
[reuse-3.3]: https://reuse.software/spec-3.3/
[linux-dco]: https://developercertificate.org/
