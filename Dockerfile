# syntax=docker/dockerfile:1
# Stage 1: build the React frontend
FROM node:20-slim AS frontend-build
WORKDIR /frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm install --legacy-peer-deps
COPY frontend/ ./
RUN npm run build

# Stage 2: Python API + compiled frontend static files
FROM python:3.12-slim
WORKDIR /app
COPY backend/pyproject.toml .
RUN pip install --no-cache-dir -e ".[dev]" 2>/dev/null || pip install --no-cache-dir .
COPY backend/ .
RUN pip install --no-cache-dir -e .

# Plugins to include, as space-separated pip requirements, e.g.
#   --build-arg PENNYCHEST_PLUGINS="git+https://github.com/pennychest/pennychest-plugins@<tag>#subdirectory=hsbc"
# git is only needed to fetch the plugins, so it's removed again in the same layer.
ARG PENNYCHEST_PLUGINS=""
RUN if [ -n "$PENNYCHEST_PLUGINS" ]; then \
      set -e; \
      apt-get update; \
      apt-get install -y --no-install-recommends git; \
      pip install --no-cache-dir $PENNYCHEST_PLUGINS; \
      apt-get purge -y --auto-remove git; \
      rm -rf /var/lib/apt/lists/*; \
    fi

# Copy the Vite build output into the location the API serves from
COPY --from=frontend-build /frontend/dist ./static

RUN mkdir -p /data/uploads

EXPOSE 8000
CMD ["sh", "-c", "alembic upgrade head && uvicorn pennychest.main:app --host 0.0.0.0 --port 8000"]
