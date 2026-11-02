"""The two failure kinds every cluster backend raises and run.py grades by."""

from __future__ import annotations


class ClusterError(Exception):
    """The backend failed in a way that is not the system's fault."""


class StagingError(Exception):
    """The case could not be put into its starting condition."""
