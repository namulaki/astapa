package astapa

import (
	"context"
	"database/sql"
	"fmt"
	"net/http"
	"net/url"

	_ "github.com/lib/pq"
)

// SessionHandle wraps a Session with methods for interacting with the API.
type SessionHandle struct {
	Session
	client *Client
}

// ConnectionString returns a PostgreSQL connection string configured for this session.
// The connection string includes the astapa_session option to route queries through
// the session's shadow schema.
func (s *SessionHandle) ConnectionString(database, user, password string) string {
	// Build connection string with session context via options parameter
	connStr := fmt.Sprintf(
		"host=%s port=%d dbname=%s user=%s password=%s sslmode=disable options='-c astapa_session=%s'",
		s.client.proxyHost,
		s.client.proxyPort,
		database,
		user,
		password,
		s.ID,
	)
	return connStr
}

// ConnectionStringURL returns the connection string in URL format.
func (s *SessionHandle) ConnectionStringURL(database, user, password string) string {
	// URL-encode the options parameter
	options := url.QueryEscape(fmt.Sprintf("-c astapa_session=%s", s.ID))
	return fmt.Sprintf(
		"postgres://%s:%s@%s:%d/%s?sslmode=disable&options=%s",
		user,
		password,
		s.client.proxyHost,
		s.client.proxyPort,
		database,
		options,
	)
}

// Connect opens a database connection configured for this session.
// The caller is responsible for closing the returned *sql.DB.
func (s *SessionHandle) Connect(ctx context.Context, database, user, password string) (*sql.DB, error) {
	connStr := s.ConnectionString(database, user, password)
	db, err := sql.Open("postgres", connStr)
	if err != nil {
		return nil, fmt.Errorf("open database: %w", err)
	}

	// Verify connection
	if err := db.PingContext(ctx); err != nil {
		db.Close()
		return nil, fmt.Errorf("ping database: %w", err)
	}

	return db, nil
}

// Refresh updates the session data from the API.
func (s *SessionHandle) Refresh(ctx context.Context) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, s.client.apiURL+"/sessions/"+s.ID, nil)
	if err != nil {
		return fmt.Errorf("create request: %w", err)
	}

	var session Session
	if err := s.client.do(req, &session); err != nil {
		return err
	}

	s.Session = session
	return nil
}

// Diff retrieves the changes made in this session, grouped by table.
func (s *SessionHandle) Diff(ctx context.Context) (*DiffResponse, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, s.client.apiURL+"/sessions/"+s.ID+"/diff", nil)
	if err != nil {
		return nil, fmt.Errorf("create request: %w", err)
	}

	var diff DiffResponse
	if err := s.client.do(req, &diff); err != nil {
		return nil, err
	}

	return &diff, nil
}

// Mutations retrieves the chronological log of mutations in this session.
func (s *SessionHandle) Mutations(ctx context.Context) (*MutationsResponse, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, s.client.apiURL+"/sessions/"+s.ID+"/mutations", nil)
	if err != nil {
		return nil, fmt.Errorf("create request: %w", err)
	}

	var mutations MutationsResponse
	if err := s.client.do(req, &mutations); err != nil {
		return nil, err
	}

	return &mutations, nil
}

// Propose submits the session for review.
func (s *SessionHandle) Propose(ctx context.Context) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, s.client.apiURL+"/sessions/"+s.ID+"/propose", nil)
	if err != nil {
		return fmt.Errorf("create request: %w", err)
	}

	if err := s.client.do(req, &s.Session); err != nil {
		return err
	}

	return nil
}

// Approve applies the changes in this session to the main database.
func (s *SessionHandle) Approve(ctx context.Context) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, s.client.apiURL+"/sessions/"+s.ID+"/approve", nil)
	if err != nil {
		return fmt.Errorf("create request: %w", err)
	}

	if err := s.client.do(req, &s.Session); err != nil {
		return err
	}

	return nil
}

// Reject discards the changes in this session.
func (s *SessionHandle) Reject(ctx context.Context) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, s.client.apiURL+"/sessions/"+s.ID+"/reject", nil)
	if err != nil {
		return fmt.Errorf("create request: %w", err)
	}

	if err := s.client.do(req, &s.Session); err != nil {
		return err
	}

	return nil
}

// Destroy deletes the session and cleans up associated resources.
func (s *SessionHandle) Destroy(ctx context.Context) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodDelete, s.client.apiURL+"/sessions/"+s.ID, nil)
	if err != nil {
		return fmt.Errorf("create request: %w", err)
	}

	if err := s.client.do(req, nil); err != nil {
		return err
	}

	return nil
}
