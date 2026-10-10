"""Settings → Plugins: the plugins each repository offers, and installing and removing them."""

from concurrent.futures import ThreadPoolExecutor

from fastapi import APIRouter, Depends, HTTPException, Response
from pydantic import BaseModel
from sqlalchemy.orm import Session

from pennychest.core.config import settings
from pennychest.core.database import get_db
from pennychest.plugins import installer, repositories
from pennychest.plugins.repositories import OFFICIAL, Index

router = APIRouter(prefix="/api/plugins", tags=["plugins"])


def _try_fetch(url: str) -> Index | None:
    try:
        return repositories.fetch_index(url)
    except Exception:
        return None


@router.get("")
def list_plugins(db: Session = Depends(get_db)):
    urls = repositories.repositories(db)
    with ThreadPoolExecutor() as pool:
        indexes = list(pool.map(_try_fetch, urls))
    installed = installer.installed()
    listed: set[str] = set()

    result = []
    for url, index in zip(urls, indexes):
        plugins = []
        for plugin in index.plugins if index else []:
            package = installer.canonical(plugin.package)
            listed.add(package)
            built_in = installer.built_in_version(plugin.package)
            plugins.append({
                "package": plugin.package,
                "name": plugin.name,
                "description": plugin.description,
                "version": plugin.version,
                "installed_version": built_in or installed.get(package),
                "built_in": built_in is not None,
            })
        result.append({
            "url": url,
            "name": index.name if index else url,
            "official": url == OFFICIAL,
            "error": None if index else "Couldn't read this repository's plugin list.",
            "plugins": plugins,
        })

    return {
        "repositories": result,
        # Installed from a repository that's since been removed, or that couldn't be read
        "other_installed": [
            {"package": package, "version": version}
            for package, version in installed.items()
            if package not in listed
        ],
        "can_add_repositories": settings.allow_plugin_repositories,
    }


class RepositoryIn(BaseModel):
    url: str


@router.post("/repositories", status_code=204)
def add_repository(body: RepositoryIn, db: Session = Depends(get_db)):
    if not settings.allow_plugin_repositories:
        raise HTTPException(status_code=403, detail="Adding plugin repositories is turned off.")
    try:
        url = repositories.normalise(body.url)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    if url in repositories.repositories(db):
        raise HTTPException(status_code=400, detail="That repository is already added.")
    if _try_fetch(url) is None:
        raise HTTPException(
            status_code=400,
            detail=f"Couldn't find a valid {repositories.INDEX_FILE} in that repository.",
        )
    repositories.set_added(db, [*repositories.added(db), url])
    return Response(status_code=204)


@router.delete("/repositories", status_code=204)
def remove_repository(url: str, db: Session = Depends(get_db)):
    """Plugins installed from it stay installed."""
    repositories.set_added(db, [u for u in repositories.added(db) if u != url])
    return Response(status_code=204)


class InstallIn(BaseModel):
    repository: str
    package: str


@router.post("/install", status_code=204)
def install_plugin(body: InstallIn, db: Session = Depends(get_db)):
    """Install or update a plugin, from the URL its repository lists for it now."""
    if body.repository not in repositories.repositories(db):
        raise HTTPException(status_code=400, detail="That repository isn't added.")
    index = _try_fetch(body.repository)
    if index is None:
        raise HTTPException(status_code=502, detail="Couldn't read the repository's plugin list.")
    plugin = next((p for p in index.plugins if p.package == body.package), None)
    if plugin is None:
        raise HTTPException(status_code=404, detail="The repository doesn't list that plugin.")
    if installer.built_in_version(plugin.package):
        raise HTTPException(status_code=400, detail="That plugin is built into PennyChest.")
    try:
        installer.install(db, plugin.package, f"{plugin.package} @ {plugin.url}")
    except installer.PluginInstallError as e:
        raise HTTPException(status_code=400, detail=f"Couldn't install {plugin.name}: {e}")
    return Response(status_code=204)


class UninstallIn(BaseModel):
    package: str


@router.post("/uninstall", status_code=204)
def uninstall_plugin(body: UninstallIn, db: Session = Depends(get_db)):
    if installer.canonical(body.package) not in installer.installed():
        raise HTTPException(status_code=404, detail="That plugin isn't installed from Settings.")
    try:
        installer.uninstall(db, body.package)
    except installer.PluginInstallError as e:
        raise HTTPException(status_code=400, detail=f"Couldn't remove it: {e}")
    return Response(status_code=204)
