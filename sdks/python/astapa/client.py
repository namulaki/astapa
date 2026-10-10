"""Astapa client for managing database change sessions."""

from __future__ import annotations

from contextlib import contextmanager
from typing import Any, Iterator, Optional

import requests

from .session import Session


class SessionContext:
    """Context manager for a Astapa session with auto-connection."""

    def __init__(
        self,
        session: Session,
        database: str,
        user: str,
        password: str,
        auto_propose: bool = True,
    ) -> None:
        self._session = session
        self._database = database
        self._user = user
        self._password = password
        self._auto_propose = auto_propose

    def __enter__(self) -> Session:
        self._session.connect(
            database=self._database,
            user=self._user,
            password=self._password,
        )
        return self._session

    def __exit__(
        self,
        exc_type: Optional[type[BaseException]],
        exc_val: Optional[BaseException],
        exc_tb: Any,
    ) -> None:
        self._session._close_connection()
        if exc_type is None and self._auto_propose:
            self._session.propose()


class Astapa:
    """
    Client for interacting with the Astapa proxy service.

    The Astapa proxy provides isolated database sessions for reviewing
    changes before applying them to the main database.

    Example:
        client = Astapa(
            api_url="http://localhost:8080",
            proxy_host="localhost",
            proxy_port=5433,
        )

        # Create and use a session
        session = client.create_session(project_id="my-project")
        conn = session.connect(database="mydb", user="user", password="pass")
        # ... make changes ...
        session.propose()
        session.approve()

        # Or use context manager
        with client.session(
            project_id="demo",
            database="mydb",
            user="user",
            password="pass",
        ) as session:
            session.execute("UPDATE users SET status = 'active'")
            # auto-proposes on exit
    """

    def __init__(
        self,
        api_url: str = "http://localhost:8080",
        proxy_host: str = "localhost",
        proxy_port: int = 5433,
        timeout: float = 30.0,
    ) -> None:
        """
        Initialize the Astapa client.

        Args:
            api_url: Base URL for the Astapa API
            proxy_host: Hostname for the PostgreSQL proxy
            proxy_port: Port for the PostgreSQL proxy
            timeout: Default timeout for HTTP requests in seconds
        """
        self._api_url = api_url.rstrip("/")
        self._proxy_host = proxy_host
        self._proxy_port = proxy_port
        self._timeout = timeout

    def create_session(self, project_id: str) -> Session:
        """
        Create a new Astapa session.

        Args:
            project_id: Identifier for the project

        Returns:
            A new Session instance
        """
        response = requests.post(
            f"{self._api_url}/sessions",
            json={"project_id": project_id},
            timeout=self._timeout,
        )
        response.raise_for_status()
        data = response.json()

        return Session(
            session_id=data["id"],
            api_url=self._api_url,
            proxy_host=self._proxy_host,
            proxy_port=self._proxy_port,
            data=data,
        )

    def get_session(self, session_id: str) -> Session:
        """
        Retrieve an existing session by ID.

        Args:
            session_id: The session ID

        Returns:
            The Session instance
        """
        response = requests.get(
            f"{self._api_url}/sessions/{session_id}",
            timeout=self._timeout,
        )
        response.raise_for_status()
        data = response.json()

        return Session(
            session_id=data["id"],
            api_url=self._api_url,
            proxy_host=self._proxy_host,
            proxy_port=self._proxy_port,
            data=data,
        )

    def session(
        self,
        project_id: str,
        database: str,
        user: str,
        password: str,
        auto_propose: bool = True,
    ) -> SessionContext:
        """
        Create a session context manager with auto-connection.

        The session will automatically connect to the database on enter
        and propose changes on exit (unless an exception occurs).

        Args:
            project_id: Identifier for the project
            database: Database name to connect to
            user: Database user
            password: Database password
            auto_propose: Whether to auto-propose on successful exit

        Returns:
            A context manager that yields the Session

        Example:
            with client.session(
                project_id="demo",
                database="mydb",
                user="user",
                password="pass",
            ) as session:
                session.execute("INSERT INTO logs VALUES (...)")
                # auto-proposes on exit, requires explicit approve
        """
        sess = self.create_session(project_id)
        return SessionContext(
            session=sess,
            database=database,
            user=user,
            password=password,
            auto_propose=auto_propose,
        )

    def __repr__(self) -> str:
        return (
            f"Astapa(api_url={self._api_url!r}, "
            f"proxy_host={self._proxy_host!r}, "
            f"proxy_port={self._proxy_port})"
        )
