"""Plugin repositories: where Settings finds plugins to install.

A repository is a link, usually to a GitHub repository such as a fork of pennychest-plugins,
with a plugins.json at its root:

    {
      "name": "PennyChest plugins",
      "plugins": [
        {
          "package": "pennychest-hsbc",
          "name": "HSBC statements",
          "description": "Import HSBC PDF statements",
          "version": "0.1.0",
          "url": "https://github.com/.../archive/refs/tags/hsbc-v0.1.0.zip#subdirectory=hsbc"
        }
      ]
    }

`url` is anything pip can install from. The official repository is always offered; others are
added in Settings, unless settings.allow_plugin_repositories is off.
"""

import json
import re
from urllib.parse import urlparse

import httpx
from pydantic import BaseModel, field_validator
from sqlalchemy.orm import Session

from pennychest.core.config import settings
from pennychest.settings.models import AppSetting

OFFICIAL = "https://github.com/pennychest/pennychest-plugins"
INDEX_FILE = "plugins.json"
# [extra repository URL, ...]
REPOSITORIES_KEY = "plugins.repositories"


class IndexPlugin(BaseModel):
    package: str
    name: str
    description: str = ""
    version: str
    url: str

    @field_validator("package")
    @classmethod
    def _package(cls, value: str) -> str:
        if not re.fullmatch(r"[A-Za-z0-9]([A-Za-z0-9._-]*[A-Za-z0-9])?", value):
            raise ValueError("not a package name")
        return value

    @field_validator("url")
    @classmethod
    def _url(cls, value: str) -> str:
        if urlparse(value).scheme != "https" or any(c.isspace() for c in value):
            raise ValueError("must be an https:// URL")
        return value


class Index(BaseModel):
    name: str
    plugins: list[IndexPlugin]


def normalise(url: str) -> str:
    """A repository link in one form, so the same repository can't be added twice."""
    url = url.strip().removesuffix("/").removesuffix(".git")
    parsed = urlparse(url)
    if parsed.scheme != "https" or not parsed.netloc:
        raise ValueError("Enter an https:// link to the repository.")
    return url


def index_url(repository: str) -> str:
    """Where a repository's plugins.json is: GitHub repositories are read through
    raw.githubusercontent.com, and any other link is taken to be a folder or the file itself."""
    match = re.fullmatch(r"https://github\.com/([^/]+)/([^/]+)", repository)
    if match:
        return f"https://raw.githubusercontent.com/{match[1]}/{match[2]}/HEAD/{INDEX_FILE}"
    if repository.endswith(".json"):
        return repository
    return f"{repository}/{INDEX_FILE}"


def fetch_index(repository: str) -> Index:
    response = httpx.get(index_url(repository), timeout=10, follow_redirects=True)
    response.raise_for_status()
    return Index.model_validate(response.json())


def added(db: Session) -> list[str]:
    """The repositories added in Settings."""
    setting = db.get(AppSetting, REPOSITORIES_KEY)
    return json.loads(setting.value) if setting and setting.value else []


def set_added(db: Session, urls: list[str]) -> None:
    setting = db.get(AppSetting, REPOSITORIES_KEY)
    if setting:
        setting.value = json.dumps(urls)
    else:
        db.add(AppSetting(key=REPOSITORIES_KEY, value=json.dumps(urls)))
    db.commit()


def repositories(db: Session) -> list[str]:
    """Every repository plugins can be installed from, the official one first."""
    return [OFFICIAL, *(added(db) if settings.allow_plugin_repositories else [])]
