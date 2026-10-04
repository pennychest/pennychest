"""Exporting the user's data: a complete copy of the SQLite database, and the ledger in the
formats of the installed exporter plugins."""

import os
import sqlite3
import tempfile
from datetime import date

from fastapi import APIRouter, BackgroundTasks, Depends, HTTPException
from fastapi.responses import FileResponse, Response
from sqlalchemy.orm import Session

from pennychest.core.database import engine, get_db
from pennychest.export.base import discover_exporters, get_exporter, load_ledger

router = APIRouter(prefix="/api/export", tags=["export"])


def _sqlite_path() -> str | None:
    """The database file, when the app is running on SQLite."""
    if engine.url.get_backend_name() != "sqlite":
        return None
    return engine.url.database


@router.get("")
def export_info():
    path = _sqlite_path()
    return {
        "engine": engine.url.get_backend_name(),
        "database_backup": path is not None,
        "size_bytes": os.path.getsize(path) if path and os.path.exists(path) else None,
        "exporters": [
            {
                "name": name,
                "label": exporter.label or name,
                "description": exporter.description,
                "file_extension": exporter.file_extension,
            }
            for name, exporter in discover_exporters().items()
        ],
    }


@router.get("/database")
def download_database(background: BackgroundTasks):
    """A consistent copy of the whole database, made with SQLite's backup API so it's safe to
    take while the app is in use."""
    path = _sqlite_path()
    if path is None:
        raise HTTPException(
            status_code=400,
            detail="Database downloads are only available when PennyChest runs on SQLite. "
            "Back up Postgres with pg_dump instead.",
        )
    handle, copy = tempfile.mkstemp(suffix=".db", prefix="pennychest-backup-")
    os.close(handle)
    source = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    target = sqlite3.connect(copy)
    try:
        source.backup(target)
    finally:
        target.close()
        source.close()
    background.add_task(os.remove, copy)
    return FileResponse(
        copy,
        media_type="application/vnd.sqlite3",
        filename=f"pennychest-backup-{date.today().isoformat()}.db",
    )


# Declared after /database so that path isn't taken for an exporter's name
@router.get("/{name}")
def download_export(name: str, db: Session = Depends(get_db)):
    """The whole ledger in an installed exporter's format."""
    exporter = get_exporter(name)
    if exporter is None:
        raise HTTPException(status_code=404, detail=f"Exporter '{name}' isn't installed")
    today = date.today()
    return Response(
        exporter.export(load_ledger(db, today)),
        media_type=exporter.media_type,
        headers={
            "Content-Disposition": (
                f'attachment; filename="pennychest-{today.isoformat()}.{exporter.file_extension}"'
            )
        },
    )
