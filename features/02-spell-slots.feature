@pending
Feature: Work out spell slots

  @justifies:SPEC129
  Scenario Outline: SL1 The editions differ on when half casters start
    Given a <edition> campaign with a DM device
    And a character "Vex" with the class "Ranger" at level 1
    Then Vex has <slots> 1st-level spell slots

    Examples:
      | edition | slots |
      | 2014    | 0     |
      | 2024    | 2     |

  @justifies:SPEC132 @justifies:SPEC135
  Scenario: SL2 Multiclass casters share one pool, Pact Magic apart
    Given a 2024 campaign with a DM device
    And a character "Scanlan" with the classes "Bard" level 3 and "Warlock" level 2
    Then Scanlan has 4 1st-level and 2 2nd-level spell slots
    And Scanlan has 2 pact slots of 1st level

  @justifies:SPEC136
  Scenario: SL3 Spent slots stay spent when a level is gained
    Given a 2024 campaign with a DM device
    And a character "Keyleth" with the class "Druid" at level 2, with 2 of 3 1st-level slots spent
    When the DM runs "edda keyleth class add Druid 1"
    Then Keyleth has 2 of 4 1st-level slots spent

  @justifies:SPEC137 @justifies:SPEC143
  Scenario: SL4 Casting spends a slot until none are left
    Given a 2024 campaign with a DM device
    And a character "Keyleth" with 1 1st-level slot left and the spell "Cure Wounds" of level 1
    When the DM runs "edda keyleth cast cure wounds"
    Then Keyleth has 0 1st-level slots left
    When the DM runs "edda keyleth cast cure wounds"
    Then the exit code is not 0
    And the output says there is no 1st-level slot left

  @justifies:SPEC140 @justifies:SPEC141
  Scenario: SL5 Cantrips and rituals cost nothing
    Given a 2024 campaign with a DM device
    And a character "Keyleth" with no slots left, the cantrip "Druidcraft" and the ritual "Detect Magic"
    When the DM runs "edda keyleth cast druidcraft"
    And the DM runs "edda keyleth cast detect magic --ritual"
    Then both exit codes are 0

  @justifies:SPEC155
  Scenario: SL6 Two devices can't both spend the last slot
    Given a 2024 campaign with a DM device and a player device playing "Keyleth"
    And Keyleth has 1 1st-level slot left
    When both devices cast "Cure Wounds" as Keyleth at the same moment
    Then exactly one cast succeeds
    And the other is refused with "no 1st-level slot left"

  @justifies:SPEC161 @justifies:SPEC163 @justifies:SPEC164
  Scenario: SL7 A stale edit doesn't give back a spent slot
    Given a 2024 campaign with a DM device and a player device playing "Keyleth"
    And the DM has Keyleth's sheet open in "edda keyleth edit"
    When the player casts "Cure Wounds" as Keyleth
    And the DM saves the edited sheet
    Then the DM is told that Keyleth changed meanwhile
    And Keyleth's spent slot is still spent
