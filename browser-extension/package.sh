#!/bin/sh
set -eu

extension_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
archive="$extension_dir/csil-devtools.zip"

"$extension_dir/build.sh"
rm -f "$archive"
bsdtar -a -cf "$archive" -C "$extension_dir/dist" .
echo "Built extension archive: $archive"
