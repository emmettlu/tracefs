#!/bin/bash
# Regenerate the SDK protocol types from schema/tracefs.schema.json (needs npx and uvx).
# The schema itself comes from `UPDATE_SCHEMA=1 cargo test -p tracefs-core schema`.
set -euo pipefail
cd "$(dirname "$0")"
SCHEMA=../schema/tracefs.schema.json

npx --yes json-schema-to-typescript@16 -i $SCHEMA -o typescript/src/protocol.ts --additionalProperties=false \
	--bannerComment '/* Generated from schema/tracefs.schema.json by sdk/gen.sh; do not edit. */'

PY=python/src/tracefs_sdk/protocol.py
uvx --from datamodel-code-generator datamodel-codegen --input $SCHEMA --input-file-type jsonschema \
	--output-model-type typing.TypedDict --target-python-version 3.11 --use-title-as-name \
	--use-schema-description --use-field-description --disable-timestamp --formatters builtin \
	--custom-file-header '# Generated from schema/tracefs.schema.json by sdk/gen.sh; do not edit.' --output $PY 2>/dev/null
# typing.TypedDict supports NotRequired since 3.11: no typing_extensions dependency.
sed -i 's/^from typing_extensions import TypedDict$/from typing import TypedDict/' $PY
