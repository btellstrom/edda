@pending
Feature: Join a campaign and log in without passwords

  @justifies:SPEC706 @justifies:SPEC707
  Scenario: DE1 A setup code makes the first DM device, once
    Given a new campaign "Vox Machina" with its setup code
    When a device runs "edda auth setup <SETUP CODE> --name DM laptop"
    Then "edda auth status" says it is the campaign's DM
    When another device runs "edda auth setup <SETUP CODE> --name Thief"
    Then the exit code is not 0

  @justifies:SPEC711
  Scenario: DE2 A device logs in only to the auth service it knows
    Given a DM device in a campaign
    When the device is pointed at another auth service with the same campaign id
    Then it refuses to log in, saying the server isn't the one it knows

  @justifies:SPEC718
  Scenario: DE3 A revoked device is locked out
    Given a campaign with a DM device and a player device "Vex's laptop"
    When the DM runs "edda auth revoke Vex's laptop"
    Then within 15 minutes the player's device can no longer use the campaign

  @justifies:SPEC721
  Scenario: DE4 The last DM device can't be revoked
    Given a campaign with one DM device "DM laptop"
    When the DM runs "edda auth revoke DM laptop"
    Then the exit code is not 0
    And "DM laptop" is still a DM device

  @justifies:SPEC722
  Scenario: DE5 A campaign that lost every DM device is recovered
    Given a campaign whose only DM device is lost
    When whoever runs the server runs "auth-service setup-code <CAMPAIGN ID>"
    Then the code sets up a new DM device for the campaign
    And the campaign's characters are all still there

  @justifies:SPEC735 @justifies:SPEC736
  Scenario: DE6 A device in several campaigns uses one at a time
    Given a device in the campaigns "Vox Machina" and "Oneshot"
    When it runs "edda auth use one"
    Then "edda list" shows Oneshot's characters
    And "edda auth campaigns" marks "Oneshot" as in use

  @justifies:SPEC705
  Scenario: DE7 The device list checks every certificate
    Given a campaign with a DM device and two player devices
    When the DM runs "edda auth devices"
    Then every device is listed with its certificate checked
