@pending
Feature: Run the servers for others

  @justifies:SPEC818
  Scenario: HO1 A client asking too much is told to wait
    Given the services run with their default limits
    When one address sends 21 sign-in requests within a minute
    Then the 21st is answered 429 with a Retry-After header

  @justifies:SPEC829
  Scenario: HO2 A full campaign is told which limit it reached
    Given the services run with LIMIT_CHARACTERS=2
    And a campaign with 2 characters
    When the DM runs "edda new Pike"
    Then it is refused with 403 naming the characters limit

  @justifies:SPEC813
  Scenario: HO3 A read-only campaign can be read but not changed
    Given a campaign made read-only through the admin API
    When its DM runs "edda list"
    Then the characters are listed
    When its DM runs "edda new Pike"
    Then it is refused with 403 saying the campaign is read-only

  @justifies:SPEC815 @justifies:SPEC816
  Scenario: HO4 A campaign moves to another server whole
    Given a campaign with characters, a paused combat, plans and its own monsters on server A
    When it is exported from A and imported into B, both signing tokens with one key
    Then its devices log in to B without pairing again
    And everything is on B as it was on A

  @justifies:SPEC817
  Scenario: HO5 An import that can't all come leaves nothing behind
    Given server B already has characters of the campaign
    When the campaign is imported into B
    Then the import is refused with 409
    And nothing of the import is on B

  @justifies:SPEC801 @justifies:SPEC802 @justifies:SPEC803 @justifies:SPEC806
  Scenario: HO6 Deleting a campaign deletes all of it and nothing else
    Given two campaigns, each with devices, characters, combats, plans and monsters
    When the first is deleted with "make delete-campaign ID=<id> YES=1"
    Then nothing of the first is on any service
    And its devices can no longer log in
    And the second campaign is unchanged

  @justifies:SPEC809
  Scenario: HO7 The admin API wants its token
    Given a service with ADMIN_ADDR and ADMIN_TOKEN set
    When a request is sent to its admin address without the token
    Then it is answered 401

  @justifies:SPEC830
  Scenario: HO8 A service without its database isn't ready
    Given a running service
    When its database becomes unreachable
    Then "/health" answers "ok"
    And "/ready" answers 503

  @justifies:SPEC831 @justifies:SPEC832
  Scenario: HO9 Logs say whose a request was, never what it asked
    Given a service logging JSON
    When a DM saves a character "Secret Villain"
    Then the log has a line with the request's method, path, status and campaign
    And no log line contains "Secret Villain"

  @justifies:SPEC841
  Scenario: HO10 A lost character comes back from a backup
    Given the stack behind HTTPS with a campaign and a character "Grog"
    And a backup made with "make backup"
    When "Grog" is deleted and the backup restored
    Then "edda list" shows "Grog"

  @justifies:SPEC901
  Scenario: HO11 The API description matches the services
    When the API description is made from the services' code
    Then it is the same as "docs/openapi.json"
