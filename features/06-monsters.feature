@pending
Feature: Monsters, spells and conditions

  @justifies:SPEC502
  Scenario Outline: MO1 A campaign sees the SRD of its edition
    Given the SRD is loaded
    And a <edition> campaign with a DM device
    When the DM runs "edda monster list"
    Then <count> shared monsters are listed

    Examples:
      | edition | count |
      | 2014    | 334   |
      | 2024    | 341   |

  @justifies:SPEC503
  Scenario: MO2 Editing a shared monster saves the campaign's own copy
    Given the SRD is loaded
    And a 2024 campaign with a DM device
    When the DM edits "troll" with "edda monster edit troll", raising its hit points
    Then the campaign has its own "Troll" with the new hit points
    And the shared "Troll" is unchanged for every other campaign
    And "edda monster show troll" shows the campaign's own

  @justifies:SPEC501
  Scenario: MO3 A stat block with a typo is refused
    Given a 2024 campaign with a DM device
    And the file "troll.json" contains:
      """
      { "name": "Cave Troll", "hit_points": 84, "hit_dice": "8d10+40", "armour_class": 15 }
      """
    When the DM runs "edda monster add troll.json"
    Then the exit code is not 0
    And the output names the field "armour_class"

  @justifies:SPEC504
  Scenario: MO4 Loading the SRD again updates rather than duplicates
    Given the SRD is loaded
    When the servers are restarted, loading the SRD again
    Then each SRD monster is in the shared library once

  @justifies:SPEC505
  Scenario: MO5 A campaign's own spell shows in the fight screen
    Given a 2024 campaign with a DM device
    And the campaign has its own spell "Frost Fingers"
    When the DM types "cast frost" in a fight screen
    Then the side panel describes "Frost Fingers"
