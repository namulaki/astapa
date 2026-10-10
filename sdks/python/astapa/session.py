"""Astapa session management."""

from __future__ import annotations

from typing import Any, Optional

import psycopg2
import requests


class Session:
    """Represents a Astapa session with database connection capabilities."""

    def __init__(
        self,
        session_id: str,
        api_url: str,
        proxy_host: str,
        proxy_port: int,
        data: dict[str, Any],
    ) -> None:
        self.id = session_id
        self._api_url = api_url.rstrip("/")
        self._proxy_host = proxy_host
        self._proxy_port = proxy_port
        self._data = data
        self._connection: Optional[psycopg2.extensions.connection] = None

    @property
    def schema_name(self) -> str:
        """Return the session's schema name."""
        return self._data.get("schema_name", "")

    @property
    def status(self) -> str:
        """Return the session's current status."""
        return self._data.get("status", "")

    def _request(self, method: str, path: str, **kwargs: Any) -> requests.Response:
        """Make an HTTP request to the API."""
        url = f"{self._api_url}/sessions/{self.id}{path}"
        response = requests.request(method, url, **kwargs)
        response.raise_for_status()
        return response

    def refresh(self) -> Session:
        """Refresh session data from the server."""
        response = self._request("GET", "")
        self._data = response.json()
        return self

    def connection_string(
        self,
        database: str,
        user: str,
        password: str,
    ) -> str:
        """
        Generate a PostgreSQL connection string for this session.

        The session ID is passed via the options parameter.
        """
        return (
            f"postgresql://{user}:{password}@{self._proxy_host}:{self._proxy_port}/{database}"
            f"?options=-c%20astapa_session%3D{self.id}"
        )

    def connect(
        self,
        database: str,
        user: str,
        password: str,
        **kwargs: Any,
    ) -> psycopg2.extensions.connection:
        """
        Create a psycopg2 connection through the Astapa proxy.

        Args:
            database: Database name
            user: Database user
            password: Database password
            **kwargs: Additional arguments passed to psycopg2.connect

        Returns:
            A psycopg2 connection object
        """
        options = f"-c astapa_session={self.id}"
        if "options" in kwargs:
            options = f"{kwargs.pop('options')} {options}"

        self._connection = psycopg2.connect(
            host=self._proxy_host,
            port=self._proxy_port,
            database=database,
            user=user,
            password=password,
            options=options,
            **kwargs,
        )
        return self._connection

    def execute(self, query: str, params: Optional[tuple[Any, ...]] = None) -> Any:
        """
        Execute a query on the session's connection.

        Requires a connection to be established first via connect() or context manager.

        Args:
            query: SQL query to execute
            params: Optional query parameters

        Returns:
            Query results if any
        """
        if self._connection is None:
            raise RuntimeError(
                "No connection established. Call connect() first or use context manager."
            )

        with self._connection.cursor() as cursor:
            cursor.execute(query, params)
            self._connection.commit()
            if cursor.description:
                return cursor.fetchall()
            return None

    def diff(self) -> dict[str, Any]:
        """Get the diff of changes in this session, organized by table."""
        response = self._request("GET", "/diff")
        return response.json()

    def mutations(self) -> list[dict[str, Any]]:
        """Get the chronological log of mutations in this session."""
        response = self._request("GET", "/mutations")
        return response.json()

    def propose(self) -> Session:
        """Submit this session for review."""
        response = self._request("POST", "/propose")
        self._data = response.json()
        return self

    def approve(self) -> Session:
        """Approve and apply the changes in this session."""
        response = self._request("POST", "/approve")
        self._data = response.json()
        return self

    def reject(self) -> Session:
        """Reject and discard the changes in this session."""
        response = self._request("POST", "/reject")
        self._data = response.json()
        return self

    def destroy(self) -> None:
        """Destroy this session."""
        self._close_connection()
        requests.delete(f"{self._api_url}/sessions/{self.id}").raise_for_status()

    def _close_connection(self) -> None:
        """Close the database connection if open."""
        if self._connection is not None:
            try:
                self._connection.close()
            except Exception:
                pass
            self._connection = None

    def __repr__(self) -> str:
        return f"Session(id={self.id!r}, status={self.status!r})"
