package astapa

import "time"

// SessionStatus represents the state of a session.
type SessionStatus string

const (
	StatusActive   SessionStatus = "active"
	StatusProposed SessionStatus = "proposed"
	StatusApproved SessionStatus = "approved"
	StatusRejected SessionStatus = "rejected"
)

// Session represents a astapa session returned by the API.
type Session struct {
	ID         string        `json:"id"`
	ProjectID  string        `json:"project_id"`
	SchemaName string        `json:"schema_name"`
	Status     SessionStatus `json:"status"`
	CreatedAt  time.Time     `json:"created_at"`
	UpdatedAt  time.Time     `json:"updated_at"`
}

// CreateSessionRequest is the request body for creating a session.
type CreateSessionRequest struct {
	ProjectID string `json:"project_id"`
}

// TableDiff represents changes to a single table.
type TableDiff struct {
	Table   string `json:"table"`
	Inserts int    `json:"inserts"`
	Updates int    `json:"updates"`
	Deletes int    `json:"deletes"`
	Rows    []Row  `json:"rows,omitempty"`
}

// Row represents a single row change in a diff.
type Row struct {
	Operation string                 `json:"operation"`
	PK        map[string]interface{} `json:"pk,omitempty"`
	Before    map[string]interface{} `json:"before,omitempty"`
	After     map[string]interface{} `json:"after,omitempty"`
}

// DiffResponse is the response from the diff endpoint.
type DiffResponse struct {
	SessionID string      `json:"session_id"`
	Tables    []TableDiff `json:"tables"`
}

// Mutation represents a single mutation in the chronological log.
type Mutation struct {
	ID        int64                  `json:"id"`
	Table     string                 `json:"table"`
	Operation string                 `json:"operation"`
	PK        map[string]interface{} `json:"pk,omitempty"`
	Before    map[string]interface{} `json:"before,omitempty"`
	After     map[string]interface{} `json:"after,omitempty"`
	Timestamp time.Time              `json:"timestamp"`
}

// MutationsResponse is the response from the mutations endpoint.
type MutationsResponse struct {
	SessionID string     `json:"session_id"`
	Mutations []Mutation `json:"mutations"`
}

// APIError represents an error response from the API.
type APIError struct {
	StatusCode int    `json:"-"`
	Message    string `json:"error"`
	Details    string `json:"details,omitempty"`
}

func (e *APIError) Error() string {
	if e.Details != "" {
		return e.Message + ": " + e.Details
	}
	return e.Message
}
